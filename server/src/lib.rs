//! Remote support server. Power monitors check in here; in a session they
//! long-poll for requests to their panel, and the owner's browser reaches a
//! device's panel through this relay. Meant to run behind Cloudflare Tunnel,
//! which provides HTTPS; nothing here listens on a public port by itself.
//!
//! Device calls (`/device/…`) carry `x-device-id` and `x-device-secret`; the
//! secret is remembered (hashed) on first contact. Everything else needs the
//! admin cookie. Any path outside `/admin` and `/device` goes to the device
//! picked with "Відкрити панель".

pub mod captcha;
pub mod relay;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::Bytes;
use axum::extract::{Form, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use power_core::remote;
pub use relay::Link;
use relay::{device_status, relay, split_head, Answer};
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
const ADMIN_COOKIE: &str = "svitlo_admin";
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
        Router::new()
            .route("/device/checkin", post(checkin))
            .route("/device/next", get(next))
            .route("/device/body/{rid}", get(body))
            .route("/device/reply/{rid}", post(reply))
            .route("/admin", get(admin_index))
            .route("/admin/", get(admin_index))
            .route("/admin/index.html", get(admin_index))
            .route("/admin/admin.js", get(admin_js))
            .route("/admin/admin_bg.wasm", get(admin_wasm))
            .route("/admin/admin.wasm", get(admin_wasm))
            .route("/admin/admin.css", get(admin_css))
            .route("/admin/assets/fonts/CascadiaCode.ttf", get(admin_font))
            .route(
                "/admin/assets/fonts/CascadiaCode-OFL.txt",
                get(admin_font_ofl),
            )
            .route("/admin/login", post(login))
            .route("/admin/captcha/{id}", get(captcha_image))
            .route("/admin/devices/{id}/waiting", post(set_waiting))
            .route("/admin/devices/{id}/open", post(open_panel))
            .route("/admin/api/devices", get(device_list))
            .route("/admin/api/captcha", get(new_captcha))
            .route("/admin/devices/{id}/end", post(end_session))
            .route("/admin/devices/{id}/forget", post(forget))
            .route("/admin/devices/{id}/view", post(set_view))
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

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Equal without telling how much matched.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0, |acc, (x, y)| acc | (x ^ y))
            == 0
}

fn unix_now() -> u64 {
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

/* ---------- device side ---------- */

#[derive(Deserialize)]
struct CheckInForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
}

async fn checkin(
    State(app): State<App>,
    headers: HeaderMap,
    Form(form): Form<CheckInForm>,
) -> Response {
    let id = match app.device(&headers) {
        Ok(id) => id,
        Err(status) => return status.into_response(),
    };
    let answer = {
        let mut devices = app.0.devices.lock().unwrap();
        let device = devices.get_mut(&id).expect("registered by device()");
        device.name = form.name.chars().take(64).collect();
        device.version = form.version.chars().take(64).collect();
        serde_json::json!({ "session": device.want_session, "soon": device.waiting })
    };
    app.save();
    Json(answer).into_response()
}

async fn next(State(app): State<App>, headers: HeaderMap) -> Response {
    let id = match app.device(&headers) {
        Ok(id) => id,
        Err(status) => return status.into_response(),
    };
    let link = app.link(&id);
    link.polled();
    // The session it asked for has started.
    if let Some(device) = app.0.devices.lock().unwrap().get_mut(&id) {
        device.want_session = false;
    }
    let deadline = tokio::time::Instant::now() + POLL;
    let mut inbox = link.inbox.lock().await;
    loop {
        let ended = link.ended.notified();
        tokio::pin!(ended);
        ended.as_mut().enable();
        if link.ending.swap(false, Ordering::SeqCst) {
            return StatusCode::GONE.into_response();
        }
        tokio::select! {
            () = &mut ended => continue,
            job = tokio::time::timeout_at(deadline, inbox.recv()) => match job {
                // A browser that stopped waiting is not worth the device's time.
                Ok(Some(request)) if !link.take(&request.rid) => continue,
                Ok(Some(request)) => {
                    link.polled();
                    return Json(request).into_response();
                }
                _ => {
                    link.polled();
                    return StatusCode::NO_CONTENT.into_response();
                }
            },
        }
    }
}

async fn body(State(app): State<App>, headers: HeaderMap, Path(rid): Path<String>) -> Response {
    let id = match app.device(&headers) {
        Ok(id) => id,
        Err(status) => return status.into_response(),
    };
    match app.link(&id).bodies.lock().unwrap().remove(&rid) {
        Some(bytes) => bytes.into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The device's answer: `x-status`, then a body of header lines, an empty
/// line and the panel's response body.
async fn reply(
    State(app): State<App>,
    headers: HeaderMap,
    Path(rid): Path<String>,
    body: Bytes,
) -> Response {
    let id = match app.device(&headers) {
        Ok(id) => id,
        Err(status) => return status.into_response(),
    };
    let status = headers
        .get("x-status")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse().ok())
        .unwrap_or(502);
    let Some((head, rest)) = split_head(&body) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let raw_headers: Vec<(String, String)> = head
        .lines()
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();

    let link = app.link(&id);
    for (name, value) in &raw_headers {
        if name.eq_ignore_ascii_case("set-cookie") {
            link.update_panel_session(value);
        }
    }

    // Do NOT pass that Set-Cookie to the browser.
    let headers: Vec<(String, String)> = raw_headers
        .into_iter()
        .filter(|(name, val)| {
            if name.eq_ignore_ascii_case("set-cookie") {
                if let Some((k, _)) = val.split(';').next().and_then(|p| p.split_once('=')) {
                    if k.trim() == remote::PANEL_COOKIE {
                        return false;
                    }
                }
            }
            true
        })
        .collect();

    let answer = Answer {
        status,
        headers,
        body: body.slice(rest..),
    };
    let pending = link.waiting.lock().unwrap().remove(&rid);
    match pending {
        Some(pending) => {
            let _ = pending.answer.send(answer);
            StatusCode::NO_CONTENT.into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/* ---------- the admin Leptos CSR app and its static assets ---------- */

const INDEX_HTML: &[u8] = include_bytes!("../admin/dist/index.html");
const ADMIN_JS: &[u8] = include_bytes!("../admin/dist/admin.js");
const ADMIN_WASM: &[u8] = include_bytes!("../admin/dist/admin_bg.wasm");
const ADMIN_CSS: &[u8] = include_bytes!("../admin/dist/admin.css");
const FONT_TTF: &[u8] = include_bytes!("../admin/dist/assets/fonts/CascadiaCode.ttf");
const FONT_OFL: &[u8] = include_bytes!("../admin/dist/assets/fonts/CascadiaCode-OFL.txt");

async fn admin_index() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        INDEX_HTML,
    )
        .into_response()
}

async fn admin_js() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/javascript"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        ADMIN_JS,
    )
        .into_response()
}

async fn admin_wasm() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/wasm"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        ADMIN_WASM,
    )
        .into_response()
}

async fn admin_css() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        ADMIN_CSS,
    )
        .into_response()
}

async fn admin_font() -> Response {
    (
        [
            (header::CONTENT_TYPE, "font/ttf"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        FONT_TTF,
    )
        .into_response()
}

async fn admin_font_ofl() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        FONT_OFL,
    )
        .into_response()
}

fn json_reply(status: StatusCode, body: serde_json::Value) -> Response {
    (status, Json(body)).into_response()
}

fn ok() -> Response {
    json_reply(StatusCode::OK, serde_json::json!({}))
}

/// The answer to a call that needs the admin; the page shows the login then.
fn login_first() -> Response {
    json_reply(
        StatusCode::UNAUTHORIZED,
        serde_json::json!({ "login": true }),
    )
}

fn unknown_device() -> Response {
    json_reply(
        StatusCode::NOT_FOUND,
        serde_json::json!({ "error": "Такого пристрою вже немає." }),
    )
}

async fn new_captcha(State(app): State<App>) -> Response {
    let id = app
        .0
        .captchas
        .lock()
        .unwrap()
        .issue(&mut rand::rng(), Instant::now());
    (
        [(header::CACHE_CONTROL, "no-store")],
        Json(serde_json::json!({ "id": id })),
    )
        .into_response()
}

async fn captcha_image(State(app): State<App>, Path(id): Path<String>) -> Response {
    match app.0.captchas.lock().unwrap().image(&id) {
        Some(svg) => (
            [
                (header::CONTENT_TYPE, "image/svg+xml"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            svg,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(Deserialize)]
struct LoginForm {
    password: String,
    #[serde(default)]
    captcha_id: String,
    #[serde(default)]
    captcha: String,
}

async fn login(State(app): State<App>, Form(form): Form<LoginForm>) -> Response {
    let solved =
        app.0
            .captchas
            .lock()
            .unwrap()
            .check(&form.captcha_id, &form.captcha, Instant::now());
    let failure = if !solved {
        Some(("captcha", "Цифри з картинки не збіглися. Введіть нові."))
    } else if !same(
        &sha256(&form.password),
        &sha256(&app.0.config.admin_password),
    ) {
        Some(("password", "Невірний пароль."))
    } else {
        None
    };
    if let Some((field, error)) = failure {
        tokio::time::sleep(Duration::from_secs(1)).await;
        return json_reply(
            StatusCode::FORBIDDEN,
            serde_json::json!({ "field": field, "error": error }),
        );
    }
    let token = random_hex(16);
    app.0.admins.lock().unwrap().insert(token.clone());
    let cookie = format!("{ADMIN_COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Lax");
    ([(header::SET_COOKIE, cookie)], ok()).into_response()
}

/// A device as the list shows it.
#[derive(Serialize)]
struct DeviceView {
    id: String,
    name: String,
    version: String,
    /// Seconds since its last call.
    seen: u64,
    /// In a session now.
    online: bool,
    /// A session is asked for at the next check-in.
    requested: bool,
    waiting: bool,
    status: &'static str,
    view: Option<String>,
}

async fn device_list(State(app): State<App>, headers: HeaderMap) -> Response {
    if !app.is_admin(&headers) {
        return login_first();
    }
    let now = unix_now();
    let mut devices: Vec<DeviceView> = app
        .0
        .devices
        .lock()
        .unwrap()
        .iter()
        .map(|(id, d)| {
            let seen_secs = now.saturating_sub(d.last_seen);
            let link = app.link(id);
            let online = link.online();
            DeviceView {
                id: id.clone(),
                name: d.name.clone(),
                version: d.version.clone(),
                seen: seen_secs,
                online,
                requested: d.want_session,
                waiting: d.waiting,
                status: device_status(online, Duration::from_secs(seen_secs)),
                view: link.get_view(),
            }
        })
        .collect();
    devices.sort_by_key(|d| d.seen);
    ([(header::CACHE_CONTROL, "no-store")], Json(devices)).into_response()
}

#[derive(Deserialize)]
struct ViewForm {
    #[serde(default)]
    view: String,
}

const ALLOWED_VIEWS: [&str; 6] = ["login", "wifi", "move", "bot", "dashboard", "bms"];

async fn set_view(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<ViewForm>,
) -> Response {
    if !app.is_admin(&headers) {
        return login_first();
    }
    if !ALLOWED_VIEWS.contains(&form.view.as_str()) {
        return StatusCode::BAD_REQUEST.into_response();
    }
    if !app.0.devices.lock().unwrap().contains_key(&id) {
        return unknown_device();
    }
    app.link(&id).set_view(Some(form.view));
    ok()
}

#[derive(Deserialize)]
struct WaitingForm {
    #[serde(default)]
    on: String,
}

async fn set_waiting(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Form(form): Form<WaitingForm>,
) -> Response {
    if !app.is_admin(&headers) {
        return login_first();
    }
    match app.0.devices.lock().unwrap().get_mut(&id) {
        Some(device) => {
            device.waiting = form.on == "1";
            // Nobody waits any more, so the session asked for is not wanted either.
            device.want_session &= device.waiting;
        }
        None => return unknown_device(),
    }
    app.save();
    ok()
}

/// Picks the device for the relay and asks for a session if it is not in one.
async fn open_panel(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !app.is_admin(&headers) {
        return login_first();
    }
    let online = app.link(&id).online();
    match app.0.devices.lock().unwrap().get_mut(&id) {
        Some(device) => device.want_session |= !online,
        None => return unknown_device(),
    }
    app.save();
    let pick = format!("{DEVICE_COOKIE}={id}; Path=/; HttpOnly; Secure; SameSite=Lax");
    (
        [(header::SET_COOKIE, pick)],
        Json(serde_json::json!({ "online": online })),
    )
        .into_response()
}

async fn end_session(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if !app.is_admin(&headers) {
        return login_first();
    }
    match app.0.devices.lock().unwrap().get_mut(&id) {
        Some(device) => device.want_session = false,
        None => return unknown_device(),
    }
    app.link(&id).end();
    app.save();
    ok()
}

async fn forget(State(app): State<App>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if !app.is_admin(&headers) {
        return login_first();
    }
    app.0.devices.lock().unwrap().remove(&id);
    app.0.links.lock().unwrap().remove(&id);
    app.save();
    ok()
}
