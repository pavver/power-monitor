//! HTTP server: the single-page UI, its JSON API, and a catch-all redirect so
//! captive-portal checks land on the page. When a password is set, every API
//! call except login needs an open session.

use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use embedded_svc::http::{Headers, Method};
use embedded_svc::io::{Read, Write};
use esp_idf_svc::http::server::{Configuration, EspHttpConnection, EspHttpServer, Request};
use power_core::api::Link;
use power_core::auth::{expired_cookie, NewPassword, PasswordHash, Sessions};
use power_core::form;
use power_core::message::test_text;
use power_core::portal::redirect_target;
use power_core::settings::{Bot, BotToken, Messages, WifiCredentials};
use serde::Serialize;

use crate::network::Command;
use crate::ota;
use crate::settings::Settings;
use crate::status::Status;
use crate::telegram;

/// ui/index.html, gzipped at build time (see build.rs).
const PAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/index.html.gz"));
/// Cascadia Code cut down to the page's characters (see ui/make-font.py).
const FONT: &[u8] = include_bytes!("../ui/operator-mono.woff");
const MAX_BODY: usize = 4096;
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
) -> anyhow::Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&Configuration {
        uri_match_wildcard: true,
        // Bot setup runs TLS handshakes inside handlers.
        stack_size: 14 * 1024,
        max_uri_handlers: 20,
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

    let a = auth.clone();
    server.fn_handler::<anyhow::Error, _>("/api/login", Method::Post, move |mut req| {
        let reply = read_body(&mut req).and_then(|body| login(&a, &body));
        respond(req, reply)
    })?;

    let a = auth.clone();
    server.fn_handler::<anyhow::Error, _>("/api/logout", Method::Post, move |req| {
        a.sessions.lock().unwrap().close(req.header("Cookie"));
        respond(req, Ok(Answer { body: json_text(&"bye"), cookie: Some(expired_cookie()) }))
    })?;

    let (s, st) = (settings.clone(), status.clone());
    get(&mut server, &auth, "/api/state", move || plain(st.state_json(&s)))?;

    let (st, net) = (status.clone(), network.clone());
    get(&mut server, &auth, "/api/networks", move || {
        let _ = net.send(Command::Scan);
        json(&st.networks())
    })?;

    let net = network.clone();
    post(&mut server, &auth, "/api/setup/close", move |_| {
        net.send(Command::CloseSetup).map_err(|e| e.to_string())?;
        json(&"closing")
    })?;

    let (st, net) = (status.clone(), network.clone());
    post(&mut server, &auth, "/api/wifi", move |body| {
        let credentials = WifiCredentials::from_form(body).map_err(|e| e.to_string())?;
        // Set before replying, so the page never mistakes the previous
        // attempt's result for this one's.
        st.set_link(Link::Connecting { ssid: credentials.ssid().to_string() });
        net.send(Command::Connect(credentials)).map_err(|e| e.to_string())?;
        json(&"connecting")
    })?;

    post(&mut server, &auth, "/api/bot/check", |body| {
        let username = telegram::bot_username(&token(body)?)?;
        json(&serde_json::json!({ "username": username }))
    })?;

    post(&mut server, &auth, "/api/bot/chats", |body| json(&telegram::chats(&token(body)?)?))?;

    let s = settings.clone();
    post(&mut server, &auth, "/api/bot", move |body| {
        let bot = Bot::from_form(body).map_err(|e| e.to_string())?;
        telegram::send(&bot, &test_text(s.messages().device_name()))?;
        s.save_bot(bot).map_err(|e| e.to_string())?;
        if !s.stage().needs_setup_ap() {
            let _ = network.send(Command::CloseSetup);
        }
        json(&"saved")
    })?;

    let s = settings.clone();
    post(&mut server, &auth, "/api/messages", move |body| {
        let messages = Messages::from_form(body).map_err(|e| e.to_string())?;
        s.save_messages(messages).map_err(|e| e.to_string())?;
        json(&"saved")
    })?;

    let s = settings.clone();
    post(&mut server, &auth, "/api/test", move |_| {
        let bot = s.bot().ok_or("Спершу налаштуйте бота.")?;
        telegram::send(&bot, &test_text(s.messages().device_name()))?;
        json(&"sent")
    })?;

    let a = auth.clone();
    post(&mut server, &auth, "/api/password", move |body| {
        let password = NewPassword::from_form(body).map_err(|e| e.to_string())?;
        a.settings.save_password(Some(PasswordHash::new(&password, random()))).map_err(|e| e.to_string())?;
        // Everyone else has to log in with the new password; this browser stays in.
        a.sessions.lock().unwrap().close_all();
        Ok(Answer { body: json_text(&"saved"), cookie: Some(a.open_session()) })
    })?;

    let a = auth.clone();
    post(&mut server, &auth, "/api/password/off", move |_| {
        a.settings.save_password(None).map_err(|e| e.to_string())?;
        a.sessions.lock().unwrap().close_all();
        json(&"off")
    })?;

    let s = settings.clone();
    post(&mut server, &auth, "/api/factory-reset", move |_| {
        s.factory_reset().map_err(|e| e.to_string())?;
        restart_soon();
        json(&"reset")
    })?;

    let a = auth.clone();
    server.fn_handler::<anyhow::Error, _>("/api/ota", Method::Post, move |mut req| {
        if !a.allows(req.header("Cookie")) {
            return deny(req);
        }
        let size = req.content_len().unwrap_or(0) as usize;
        let reply = ota::update(&mut req, size).and_then(|info| {
            restart_soon();
            json(&serde_json::json!({ "version": info.version, "built": info.built }))
        });
        respond(req, reply)
    })?;

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

fn get<F>(server: &mut EspHttpServer<'static>, auth: &Arc<Auth>, uri: &str, handler: F) -> anyhow::Result<()>
where
    F: Fn() -> Reply + Send + Sync + 'static,
{
    let auth = auth.clone();
    server.fn_handler::<anyhow::Error, _>(uri, Method::Get, move |req| {
        if !auth.allows(req.header("Cookie")) {
            return deny(req);
        }
        respond(req, handler())
    })?;
    Ok(())
}

fn post<F>(server: &mut EspHttpServer<'static>, auth: &Arc<Auth>, uri: &str, handler: F) -> anyhow::Result<()>
where
    F: Fn(&str) -> Reply + Send + Sync + 'static,
{
    let auth = auth.clone();
    server.fn_handler::<anyhow::Error, _>(uri, Method::Post, move |mut req| {
        if !auth.allows(req.header("Cookie")) {
            return deny(req);
        }
        let reply = read_body(&mut req).and_then(|body| handler(&body));
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

fn read_body(req: &mut Req) -> Result<String, String> {
    let len = req.content_len().unwrap_or(0) as usize;
    if len > MAX_BODY {
        return Err("Запит завеликий.".to_string());
    }
    let mut body = vec![0; len];
    req.read_exact(&mut body).map_err(|e| format!("{e:?}"))?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Hardware random bytes (the RF subsystem is running, so they are true random).
fn random() -> [u8; 16] {
    let mut bytes = [0u8; 16];
    // SAFETY: writes exactly `bytes.len()` bytes into the buffer.
    unsafe { esp_idf_svc::sys::esp_fill_random(bytes.as_mut_ptr().cast(), bytes.len()) };
    bytes
}

/// Lets the response reach the browser before the reboot.
fn restart_soon() {
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(2));
        esp_idf_svc::hal::reset::restart();
    });
}
