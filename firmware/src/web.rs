//! HTTP server: the single-page UI, its JSON API, and a catch-all redirect so
//! captive-portal checks land on the page. When a password is set, every API
//! call except login needs an open session.

mod routes;

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use embedded_svc::http::{Headers, Method};
use embedded_svc::io::{Read, Write};
use esp_idf_svc::http::server::{Configuration, EspHttpConnection, EspHttpServer, Request};
use power_core::auth::Sessions;
use power_core::form;
use power_core::template;
use power_core::portal::redirect_target;
use power_core::settings::BotToken;
use serde::Serialize;

use crate::bms::Bms;
use crate::network::Command;
use crate::random;
use crate::remote::Remote;
use crate::settings::Settings;
use crate::status::Status;
use crate::voltmeter::Voltmeter;

/// ui/index.html, gzipped at build time (see build.rs).
const PAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/index.html.gz"));
/// Cascadia Code cut down to the page's characters (see ui/make-font.py).
const FONT: &[u8] = include_bytes!("../ui/operator-mono.woff");
/// A form: the largest is a new password typed twice, 64 characters each,
/// which the page sends as %XX, up to 12 bytes a character.
pub(super) const MAX_FORM: usize = 2048;
const MAX_SESSIONS: usize = 8;
/// Slows down password guessing.
const FAILED_LOGIN_DELAY: Duration = Duration::from_secs(1);

type Req<'a, 'b> = Request<&'a mut EspHttpConnection<'b>>;

/// A JSON body, optionally with a cookie to set.
struct Answer {
    body: String,
    cookie: Option<String>,
}

/// An answer, or a message for the user shown with status 400.
type Reply = Result<Answer, String>;

struct Auth {
    settings: Arc<Settings>,
    sessions: Mutex<Sessions>,
}

impl Auth {
    fn allows(&self, cookie_header: Option<&str>) -> bool {
        self.settings.password().is_none() || self.sessions.lock().unwrap().is_open(cookie_header)
    }

    /// Opens a session and returns its `Set-Cookie` value.
    fn open_session(&self) -> String {
        self.sessions.lock().unwrap().open(random())
    }
}

pub fn start(
    settings: Arc<Settings>,
    status: Arc<Status>,
    network: Sender<Command>,
    bms: Bms,
    voltmeter: Voltmeter,
    remote: Remote,
) -> anyhow::Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&Configuration {
        uri_match_wildcard: true,
        // Bot setup runs TLS handshakes inside handlers.
        stack_size: 14 * 1024,
        max_uri_handlers: 30,
        // Phones open several connections at once while probing the portal.
        max_open_sockets: 7,
        ..Default::default()
    })?;
    let auth = Arc::new(Auth { settings: settings.clone(), sessions: Mutex::new(Sessions::new(MAX_SESSIONS)) });

    server.fn_handler::<anyhow::Error, _>("/", Method::Get, |req| {
        let headers = [("Content-Type", "text/html; charset=utf-8"), ("Content-Encoding", "gzip")];
        req.into_response(200, None, &headers)?.write_all(PAGE)?;
        Ok(())
    })?;

    server.fn_handler::<anyhow::Error, _>("/operator-mono.woff", Method::Get, |req| {
        let headers = [("Content-Type", "font/woff"), ("Cache-Control", "public, max-age=31536000, immutable")];
        req.into_response(200, None, &headers)?.write_all(FONT)?;
        Ok(())
    })?;

    let device = Arc::new(routes::Device { settings, status, network, bms, voltmeter, remote });
    routes::setup(&mut server, &auth, &device)?;
    routes::bot(&mut server, &auth, &device)?;
    routes::battery(&mut server, &auth, &device)?;
    routes::remote(&mut server, &auth, &device)?;
    routes::access(&mut server, &auth, &device)?;

    // Connectivity probes (generate_204, hotspot-detect.html, …) land here.
    server.fn_handler::<anyhow::Error, _>("/*", Method::Get, |req| {
        let location = redirect_target(req.header("Host"));
        req.into_response(302, None, &[("Location", &location), ("Cache-Control", "no-store")])?;
        Ok(())
    })?;

    Ok(server)
}

fn login(auth: &Auth, body: &str) -> Reply {
    let attempt = form::field(&form::parse(body), "password").to_string();
    match auth.settings.password() {
        Some(hash) if !hash.verify(&attempt) => {
            thread::sleep(FAILED_LOGIN_DELAY);
            Err("Невірний пароль.".to_string())
        }
        _ => Ok(Answer { body: json_text(&"ok"), cookie: Some(auth.open_session()) }),
    }
}

fn token(body: &str) -> Result<BotToken, String> {
    BotToken::parse(form::field(&form::parse(body), "token")).map_err(|e| e.to_string())
}

fn json_text<T: Serialize + ?Sized>(value: &T) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

fn json<T: Serialize + ?Sized>(value: &T) -> Reply {
    plain(json_text(value))
}

fn plain(body: String) -> Reply {
    Ok(Answer { body, cookie: None })
}

/// A route's own work. Boxed, so that the wrapper around it (password
/// check, body, answer) is compiled once, not once per route.
type Handler = Box<dyn Fn(&str, &str) -> Reply + Send + Sync>;

fn get<F>(server: &mut EspHttpServer<'static>, auth: &Arc<Auth>, uri: &str, handler: F) -> anyhow::Result<()>
where
    F: Fn() -> Reply + Send + Sync + 'static,
{
    serve_get(server, auth, uri, Box::new(move |_, _| handler()))
}

fn serve_get(server: &mut EspHttpServer<'static>, auth: &Arc<Auth>, uri: &str, handler: Handler) -> anyhow::Result<()> {
    let auth = auth.clone();
    server.fn_handler::<anyhow::Error, _>(uri, Method::Get, move |req| {
        if !auth.allows(req.header("Cookie")) {
            return deny(req);
        }
        respond(req, handler("", ""))
    })?;
    Ok(())
}

fn post<F>(server: &mut EspHttpServer<'static>, auth: &Arc<Auth>, uri: &str, handler: F) -> anyhow::Result<()>
where
    F: Fn(&str) -> Reply + Send + Sync + 'static,
{
    accept(server, auth, uri, MAX_FORM, Box::new(move |_, body| handler(body)))
}

/// Like `post`, for a body that is plain text: the handler also gets the
/// query string that names what the text is for.
/// The text is a message text, so it is held to `template::MAX_BYTES`.
fn post_text<F>(server: &mut EspHttpServer<'static>, auth: &Arc<Auth>, uri: &str, handler: F) -> anyhow::Result<()>
where
    F: Fn(&str, &str) -> Reply + Send + Sync + 'static,
{
    accept(server, auth, uri, template::MAX_BYTES, Box::new(handler))
}

fn accept(server: &mut EspHttpServer<'static>, auth: &Arc<Auth>, uri: &str, limit: usize, handler: Handler) -> anyhow::Result<()> {
    let auth = auth.clone();
    server.fn_handler::<anyhow::Error, _>(uri, Method::Post, move |mut req| {
        if !auth.allows(req.header("Cookie")) {
            return deny(req);
        }
        let query = req.uri().split_once('?').map_or("", |(_, query)| query).to_string();
        let reply = read_body(&mut req, limit).and_then(|body| handler(&query, &body));
        respond(req, reply)
    })?;
    Ok(())
}

/// 401 with a flag the page uses to show the login screen.
fn deny(req: Req) -> anyhow::Result<()> {
    let body = serde_json::json!({ "error": "Потрібен пароль.", "login": true }).to_string();
    let headers = [("Content-Type", "application/json"), ("Cache-Control", "no-store")];
    req.into_response(401, None, &headers)?.write_all(body.as_bytes())?;
    Ok(())
}

fn respond(req: Req, reply: Reply) -> anyhow::Result<()> {
    let (code, answer) = match reply {
        Ok(answer) => (200, answer),
        Err(message) => (400, Answer { body: serde_json::json!({ "error": message }).to_string(), cookie: None }),
    };
    let mut headers = vec![("Content-Type", "application/json"), ("Cache-Control", "no-store")];
    if let Some(cookie) = &answer.cookie {
        headers.push(("Set-Cookie", cookie));
    }
    req.into_response(code, None, &headers)?.write_all(answer.body.as_bytes())?;
    Ok(())
}

fn read_body(req: &mut Req, limit: usize) -> Result<String, String> {
    let len = req.content_len().unwrap_or(0) as usize;
    if len > limit {
        return Err("Запит завеликий.".to_string());
    }
    let mut body = vec![0; len];
    req.read_exact(&mut body).map_err(|e| format!("{e:?}"))?;
    Ok(String::from_utf8(body).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// Health of a device that may be out of reach of a serial cable: memory,
/// why it last restarted, and how long ago.
fn system_view() -> serde_json::Value {
    use esp_idf_svc::sys::{
        esp_get_free_heap_size, esp_get_minimum_free_heap_size, heap_caps_get_largest_free_block, MALLOC_CAP_8BIT,
    };
    // SAFETY: plain reads of heap statistics.
    let (free, min_free, largest) = unsafe {
        (esp_get_free_heap_size(), esp_get_minimum_free_heap_size(), heap_caps_get_largest_free_block(MALLOC_CAP_8BIT))
    };
    serde_json::json!({
        "heap_free": free,
        "heap_min_free": min_free,
        "heap_largest_block": largest,
        "reset_reason": format!("{:?}", esp_idf_svc::hal::reset::ResetReason::get()),
        "uptime_s": crate::status::now_ms() / 1000,
    })
}

/// A restart the owner asked for: the next start goes on from the power
/// state seen now instead of announcing itself in Telegram.
fn planned_restart(settings: &Settings, status: &Status) {
    if let Some(state) = status.power() {
        if let Err(error) = settings.keep_power_for_restart(state) {
            log::error!("keeping the power state for the restart: {error}");
        }
    }
    restart_soon();
}

/// Lets the response reach the browser before the reboot.
fn restart_soon() {
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(2));
        esp_idf_svc::hal::reset::restart();
    });
}
