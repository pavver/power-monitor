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

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use axum::body::{to_bytes, Body, Bytes};
use axum::extract::{Form, Path, Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use power_core::remote::{self, TunnelRequest};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, oneshot};

/// How long a long poll is held; well under Cloudflare's 100 s limit.
pub const POLL: Duration = Duration::from_secs(25);
/// How long the browser waits for the device's answer (a firmware upload is the slowest).
pub const ANSWER: Duration = Duration::from_secs(95);
/// A device that polled this recently is in a session.
const ONLINE: Duration = Duration::from_secs(40);
const ADMIN_COOKIE: &str = "svitlo_admin";
const DEVICE_COOKIE: &str = "svitlo_device";

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

/// A relayed request on its way to the device.
enum Job {
    Request(TunnelRequest),
    End,
}

/// The device's answer: status, headers, body.
struct Answer {
    status: u16,
    headers: Vec<(String, String)>,
    body: Bytes,
}

/// One device's session plumbing.
struct Link {
    jobs: mpsc::UnboundedSender<Job>,
    inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<Job>>,
    bodies: Mutex<HashMap<String, Bytes>>,
    waiting: Mutex<HashMap<String, oneshot::Sender<Answer>>>,
    last_poll: Mutex<Option<Instant>>,
    ending: AtomicBool,
}

impl Link {
    fn new() -> Self {
        let (jobs, inbox) = mpsc::unbounded_channel();
        Self {
            jobs,
            inbox: tokio::sync::Mutex::new(inbox),
            bodies: Mutex::default(),
            waiting: Mutex::default(),
            last_poll: Mutex::default(),
            ending: AtomicBool::new(false),
        }
    }

    fn online(&self) -> bool {
        self.last_poll.lock().unwrap().is_some_and(|at| at.elapsed() < ONLINE)
    }
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
            .route("/admin", get(admin_page))
            .route("/admin/login", post(login))
            .route("/admin/captcha/{id}", get(captcha_image))
            .route("/admin/devices/{id}/waiting", post(set_waiting))
            .route("/admin/devices/{id}/open", get(open_panel))
            .route("/admin/devices/{id}/end", post(end_session))
            .route("/admin/devices/{id}/forget", post(forget))
            .fallback(relay)
            .with_state(self)
    }

    fn link(&self, id: &str) -> Arc<Link> {
        self.0.links.lock().unwrap().entry(id.to_string()).or_insert_with(|| Arc::new(Link::new())).clone()
    }

    fn save(&self) {
        let Some(dir) = &self.0.config.data else { return };
        let json = serde_json::to_vec_pretty(&*self.0.devices.lock().unwrap()).unwrap_or_default();
        let temp = dir.join("devices.json.tmp");
        if let Err(error) = std::fs::write(&temp, json).and_then(|()| std::fs::rename(&temp, dir.join("devices.json"))) {
            eprintln!("saving devices: {error}");
        }
    }

    /// The device making this call, registering it on first contact.
    fn device(&self, headers: &HeaderMap) -> Result<String, StatusCode> {
        let field = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or("");
        let identity = format!("id={}&secret={}", field("x-device-id"), field("x-device-secret"));
        let identity = remote::Identity::from_form(&identity).ok_or(StatusCode::FORBIDDEN)?;
        let hash = sha256(&identity.secret);
        let mut devices = self.0.devices.lock().unwrap();
        let device = devices.entry(identity.id.clone()).or_insert_with(|| Device { secret_hash: hash.clone(), ..Device::default() });
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

    fn is_admin(&self, headers: &HeaderMap) -> bool {
        cookie(headers, ADMIN_COOKIE).is_some_and(|token| self.0.admins.lock().unwrap().contains(token))
    }
}

fn sha256(text: &str) -> String {
    Sha256::digest(text.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}

/// Equal without telling how much matched.
fn same(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn unix_now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn random_hex(bytes: usize) -> String {
    (0..bytes).map(|_| format!("{:02x}", rand::random::<u8>())).collect()
}

fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
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

async fn checkin(State(app): State<App>, headers: HeaderMap, Form(form): Form<CheckInForm>) -> Response {
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
    *link.last_poll.lock().unwrap() = Some(Instant::now());
    // The session it asked for has started.
    if let Some(device) = app.0.devices.lock().unwrap().get_mut(&id) {
        device.want_session = false;
    }
    if link.ending.swap(false, Ordering::SeqCst) {
        return StatusCode::GONE.into_response();
    }
    let job = {
        let mut inbox = link.inbox.lock().await;
        tokio::time::timeout(POLL, inbox.recv()).await
    };
    *link.last_poll.lock().unwrap() = Some(Instant::now());
    match job {
        Ok(Some(Job::Request(request))) => Json(request).into_response(),
        Ok(Some(Job::End)) => {
            link.ending.store(false, Ordering::SeqCst);
            StatusCode::GONE.into_response()
        }
        _ => StatusCode::NO_CONTENT.into_response(),
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
async fn reply(State(app): State<App>, headers: HeaderMap, Path(rid): Path<String>, body: Bytes) -> Response {
    let id = match app.device(&headers) {
        Ok(id) => id,
        Err(status) => return status.into_response(),
    };
    let status = headers.get("x-status").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(502);
    let Some((head, rest)) = split_head(&body) else { return StatusCode::BAD_REQUEST.into_response() };
    let headers = head
        .lines()
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let answer = Answer { status, headers, body: body.slice(rest..) };
    match app.link(&id).waiting.lock().unwrap().remove(&rid) {
        Some(waiter) => {
            let _ = waiter.send(answer);
            StatusCode::NO_CONTENT.into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

/// The header block and where the body starts.
fn split_head(body: &[u8]) -> Option<(String, usize)> {
    let end = if body.first() == Some(&b'\n') { 0 } else { body.windows(2).position(|w| w == b"\n\n")? + 1 };
    Some((String::from_utf8_lossy(&body[..end]).into_owned(), end + 1))
}

/* ---------- the owner's browser ---------- */

async fn relay(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if !app.is_admin(&headers) {
        return Redirect::to("/admin").into_response();
    }
    let Some(id) = cookie(&headers, DEVICE_COOKIE).map(str::to_string) else {
        return Redirect::to("/admin").into_response();
    };
    let link = app.link(&id);
    if !link.online() {
        return Redirect::to(&format!("/admin/devices/{id}/open")).into_response();
    }
    let method = match *request.method() {
        Method::GET => remote::Method::Get,
        Method::POST => remote::Method::Post,
        _ => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
    };
    let path = request.uri().path_and_query().map_or("/", |p| p.as_str()).to_string();
    let content_type = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let body = match to_bytes(request.into_body(), remote::MAX_BODY).await {
        Ok(body) => body,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "Завеликий запит.").into_response(),
    };
    let rid = random_hex(8);
    let (answer, answered) = oneshot::channel();
    link.waiting.lock().unwrap().insert(rid.clone(), answer);
    if !body.is_empty() {
        link.bodies.lock().unwrap().insert(rid.clone(), body.clone());
    }
    let job = TunnelRequest {
        rid: rid.clone(),
        method,
        path,
        // Only the panel's own session cookie; this server's cookies stay here.
        cookie: cookie(&headers, remote::PANEL_COOKIE).map(|token| format!("{}={token}", remote::PANEL_COOKIE)),
        content_type,
        body_len: body.len(),
    };
    let _ = link.jobs.send(Job::Request(job));
    let result = tokio::time::timeout(ANSWER, answered).await;
    link.waiting.lock().unwrap().remove(&rid);
    link.bodies.lock().unwrap().remove(&rid);
    match result {
        Ok(Ok(answer)) => {
            let mut response = Response::new(Body::from(answer.body));
            *response.status_mut() = StatusCode::from_u16(answer.status).unwrap_or(StatusCode::BAD_GATEWAY);
            for (name, value) in answer.headers {
                if let (Ok(name), Ok(value)) = (header::HeaderName::try_from(name), HeaderValue::try_from(value)) {
                    response.headers_mut().append(name, value);
                }
            }
            response
        }
        _ => (StatusCode::GATEWAY_TIMEOUT, "Пристрій не відповів.").into_response(),
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
    let solved = app.0.captchas.lock().unwrap().check(&form.captcha_id, &form.captcha, Instant::now());
    let failure = if !solved {
        Some("/admin?captcha")
    } else if !same(&sha256(&form.password), &sha256(&app.0.config.admin_password)) {
        Some("/admin?wrong")
    } else {
        None
    };
    if let Some(page) = failure {
        tokio::time::sleep(Duration::from_secs(1)).await;
        return Redirect::to(page).into_response();
    }
    let token = random_hex(16);
    app.0.admins.lock().unwrap().insert(token.clone());
    let cookie = format!("{ADMIN_COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Lax");
    ([(header::SET_COOKIE, cookie)], Redirect::to("/admin")).into_response()
}

#[derive(Deserialize)]
struct WaitingForm {
    #[serde(default)]
    on: String,
}

async fn set_waiting(State(app): State<App>, headers: HeaderMap, Path(id): Path<String>, Form(form): Form<WaitingForm>) -> Response {
    if !app.is_admin(&headers) {
        return Redirect::to("/admin").into_response();
    }
    if let Some(device) = app.0.devices.lock().unwrap().get_mut(&id) {
        device.waiting = form.on == "1";
    }
    app.save();
    Redirect::to("/admin").into_response()
}

/// Picks the device for the relay; asks for a session if it is not in one.
async fn open_panel(State(app): State<App>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if !app.is_admin(&headers) {
        return Redirect::to("/admin").into_response();
    }
    let name = {
        let mut devices = app.0.devices.lock().unwrap();
        let Some(device) = devices.get_mut(&id) else { return Redirect::to("/admin").into_response() };
        if !app.link(&id).online() {
            device.want_session = true;
        }
        device.name.clone()
    };
    app.save();
    let pick = format!("{DEVICE_COOKIE}={id}; Path=/; HttpOnly; Secure; SameSite=Lax");
    if app.link(&id).online() {
        return ([(header::SET_COOKIE, pick)], Redirect::to("/")).into_response();
    }
    let page = format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'>\
         <meta http-equiv=refresh content=5><title>Очікую пристрій</title>{STYLE}\
         <h1>Очікую «{}»</h1><p>Сесію запитано. Пристрій відкриє її на найближчій перевірці: \
         за кілька хвилин, якщо увімкнено «Чекаю пристрій», інакше до години. \
         Сторінка оновлюється сама й відкриє панель, щойно пристрій з'явиться.</p><p><a href=/admin>← до списку</a></p>",
        escape(&name)
    );
    ([(header::SET_COOKIE, pick)], Html(page)).into_response()
}

async fn end_session(State(app): State<App>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if !app.is_admin(&headers) {
        return Redirect::to("/admin").into_response();
    }
    if let Some(device) = app.0.devices.lock().unwrap().get_mut(&id) {
        device.want_session = false;
    }
    let link = app.link(&id);
    link.ending.store(true, Ordering::SeqCst);
    let _ = link.jobs.send(Job::End);
    app.save();
    Redirect::to("/admin").into_response()
}

async fn forget(State(app): State<App>, headers: HeaderMap, Path(id): Path<String>) -> Response {
    if !app.is_admin(&headers) {
        return Redirect::to("/admin").into_response();
    }
    app.0.devices.lock().unwrap().remove(&id);
    app.0.links.lock().unwrap().remove(&id);
    app.save();
    Redirect::to("/admin").into_response()
}

const STYLE: &str = "<style>body{font:15px/1.4 system-ui,sans-serif;max-width:900px;margin:24px auto;padding:0 16px}\
table{border-collapse:collapse;width:100%}td,th{border-bottom:1px solid #ddd;padding:8px;text-align:left;vertical-align:top}\
form{display:inline}button{margin:2px 4px 2px 0}small{color:#666}.on{color:#1a7f37;font-weight:600}</style>";

async fn captcha_image(State(app): State<App>, Path(id): Path<String>) -> Response {
    match app.0.captchas.lock().unwrap().image(&id) {
        Some(svg) => ([(header::CONTENT_TYPE, "image/svg+xml"), (header::CACHE_CONTROL, "no-store")], svg).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn admin_page(State(app): State<App>, headers: HeaderMap, request: Request) -> Response {
    if !app.is_admin(&headers) {
        let problem = match request.uri().query() {
            Some("wrong") => "<p style=color:#cf222e>Невірний пароль.</p>",
            Some("captcha") => "<p style=color:#cf222e>Невірні цифри з картинки.</p>",
            _ => "",
        };
        let id = app.0.captchas.lock().unwrap().issue(&mut rand::rng(), Instant::now());
        return Html(format!(
            "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'><title>Вхід</title>{STYLE}\
             <h1>Віддалена підтримка</h1>{problem}<form method=post action=/admin/login>\
             <input type=password name=password autofocus placeholder=Пароль> \
             <img src=/admin/captcha/{id} alt=Капча width=200 height=64> \
             <input type=hidden name=captcha_id value={id}><input name=captcha inputmode=numeric autocomplete=off placeholder=Цифри> \
             <button>Увійти</button></form>"
        ))
        .into_response();
    }
    let mut devices: Vec<(String, Device)> = app.0.devices.lock().unwrap().clone().into_iter().collect();
    devices.sort_by_key(|(_, d)| std::cmp::Reverse(d.last_seen));
    let now = unix_now();
    let rows: String = devices
        .iter()
        .map(|(id, d)| {
            let online = app.link(id).online();
            let state = if online {
                "<span class=on>сесія відкрита</span>".to_string()
            } else if d.want_session {
                "сесію запитано".to_string()
            } else {
                "—".to_string()
            };
            let toggle = if d.waiting { ("", "Не чекати") } else { ("1", "Чекаю пристрій") };
            format!(
                "<tr><td><b>{name}</b><br><small>{id} · {version}</small></td><td>{seen}</td><td>{state}{waiting}</td><td>\
                 <a href=/admin/devices/{id}/open><button>Відкрити панель</button></a>\
                 <form method=post action=/admin/devices/{id}/waiting><input type=hidden name=on value='{on}'><button>{label}</button></form>\
                 {end}<form method=post action=/admin/devices/{id}/forget onsubmit=\"return confirm('Забути пристрій?')\"><button>Забути</button></form></td></tr>",
                name = escape(if d.name.is_empty() { "без назви" } else { &d.name }),
                version = escape(&d.version),
                seen = ago(now.saturating_sub(d.last_seen)),
                waiting = if d.waiting { "<br><small>чекаю: перевірка кожні 2 хв</small>" } else { "" },
                on = toggle.0,
                label = toggle.1,
                end = if online {
                    format!("<form method=post action=/admin/devices/{id}/end><button>Завершити сесію</button></form>")
                } else {
                    String::new()
                },
            )
        })
        .collect();
    Html(format!(
        "<!doctype html><meta charset=utf-8><meta name=viewport content='width=device-width'>\
         <meta http-equiv=refresh content=15><title>Віддалена підтримка</title>{STYLE}\
         <h1>Пристрої</h1><table><tr><th>Пристрій</th><th>На зв'язку</th><th>Стан</th><th></th></tr>{rows}</table>\
         <p><small>Пристрій перевіряє сервер раз на годину, а з «Чекаю пристрій» — кожні 2 хв. \
         «Відкрити панель» запитує сесію й відкриває панель пристрою, щойно він з'явиться.</small></p>"
    ))
    .into_response()
}

fn ago(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds} с тому"),
        60..=3599 => format!("{} хв тому", seconds / 60),
        3600..=86_399 => format!("{} год тому", seconds / 3600),
        _ => format!("{} д тому", seconds / 86_400),
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}
