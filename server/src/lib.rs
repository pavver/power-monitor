//! Remote support server. Power monitors check in here; in a session they
//! long-poll for requests to their panel, and the owner's browser reaches a
//! device's panel through this relay. Meant to run behind Cloudflare Tunnel,
//! which provides HTTPS; nothing here listens on a public port by itself.
//!
//! Device calls (`/device/…`) carry `x-device-id` and `x-device-secret`; the
//! secret is remembered (hashed) on first contact. Everything else needs the
//! admin cookie. Any path outside `/admin` and `/device` goes to the device
//! picked with "Відкрити панель".

mod admin;
mod assets;
pub mod captcha;
mod device;
pub mod relay;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::http::{header, HeaderMap, StatusCode};
use axum::Router;
use power_core::remote;
use relay::relay;
pub use relay::Link;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// How long a long poll is held; well under Cloudflare's 100 s limit.
pub const POLL: Duration = Duration::from_secs(25);
/// How long a browser request waits for the device to take it. A device that
/// stopped polling (it is busy, e.g. scanning for a BMS, or gone) fails the
/// request fast instead of holding the page until Cloudflare cuts it.
pub const PICKUP: Duration = Duration::from_secs(15);
/// How long the browser waits for the device's answer once it took the
/// request (a firmware upload is the slowest).
pub const ANSWER: Duration = Duration::from_secs(95);
/// A device that polled this recently is in a session.
pub const ONLINE: Duration = Duration::from_secs(40);
pub(crate) const ADMIN_COOKIE: &str = "svitlo_admin";
pub const DEVICE_COOKIE: &str = "svitlo_device";

pub struct Config {
    pub admin_password: String,
    /// Where devices.json lives; `None` keeps everything in memory (tests).
    pub data: Option<PathBuf>,
}

/// What is remembered about a device.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Device {
    name: String,
    version: String,
    secret_hash: String,
    /// Unix seconds of the last call.
    last_seen: u64,
    /// The owner waits for it: it should check in often.
    waiting: bool,
    /// Open a session at the next check-in.
    want_session: bool,
}

struct Inner {
    config: Config,
    devices: Mutex<HashMap<String, Device>>,
    links: Mutex<HashMap<String, Arc<Link>>>,
    admins: Mutex<HashSet<String>>,
    captchas: Mutex<captcha::Challenges>,
}

#[derive(Clone)]
pub struct App(Arc<Inner>);

impl App {
    pub fn new(config: Config) -> Self {
        let devices = config
            .data
            .as_ref()
            .and_then(|dir| std::fs::read(dir.join("devices.json")).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self(Arc::new(Inner {
            config,
            devices: Mutex::new(devices),
            links: Mutex::default(),
            admins: Mutex::default(),
            captchas: Mutex::default(),
        }))
    }

    pub fn router(self) -> Router {
        admin::routes(device::routes(assets::routes(Router::new())))
            .fallback(relay)
            .with_state(self)
    }

    pub(crate) fn link(&self, id: &str) -> Arc<Link> {
        self.0
            .links
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_insert_with(|| Arc::new(Link::new()))
            .clone()
    }

    fn save(&self) {
        let Some(dir) = &self.0.config.data else {
            return;
        };
        let json = serde_json::to_vec_pretty(&*self.0.devices.lock().unwrap()).unwrap_or_default();
        let temp = dir.join("devices.json.tmp");
        if let Err(error) = std::fs::write(&temp, json)
            .and_then(|()| std::fs::rename(&temp, dir.join("devices.json")))
        {
            eprintln!("saving devices: {error}");
        }
    }

    /// The device making this call, registering it on first contact.
    fn device(&self, headers: &HeaderMap) -> Result<String, StatusCode> {
        let field = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
        };
        let identity = format!(
            "id={}&secret={}",
            field("x-device-id"),
            field("x-device-secret")
        );
        let identity = remote::Identity::from_form(&identity).ok_or(StatusCode::FORBIDDEN)?;
        let hash = sha256(&identity.secret);
        let mut devices = self.0.devices.lock().unwrap();
        let device = devices
            .entry(identity.id.clone())
            .or_insert_with(|| Device {
                secret_hash: hash.clone(),
                ..Device::default()
            });
        if !same(&device.secret_hash, &hash) {
            return Err(StatusCode::FORBIDDEN);
        }
        device.last_seen = unix_now();
        Ok(identity.id)
    }

    /// The answer to an open login captcha, for tests and tooling.
    pub fn captcha_answer(&self, id: &str) -> Option<String> {
        self.0.captchas.lock().unwrap().answer(id)
    }

    pub(crate) fn is_admin(&self, headers: &HeaderMap) -> bool {
        cookie(headers, ADMIN_COOKIE)
            .is_some_and(|token| self.0.admins.lock().unwrap().contains(token))
    }
}

pub(crate) fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Equal without telling how much matched.
pub(crate) fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (x, y)| acc | (x ^ y))
            == 0
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub(crate) fn random_hex(bytes: usize) -> String {
    (0..bytes)
        .map(|_| format!("{:02x}", rand::random::<u8>()))
        .collect()
}

pub(crate) fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(key, _)| *key == name)
        .map(|(_, value)| value)
}
