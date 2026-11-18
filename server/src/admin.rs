//! The admin's calls: login, the device list and what to do with a device.

use std::time::{Duration, Instant};

use axum::extract::{Form, Path, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::relay::device_status;
use crate::{random_hex, same, sha256, unix_now, App, ADMIN_COOKIE, DEVICE_COOKIE};

/// `router` with the admin's calls.
pub(crate) fn routes(router: Router<App>) -> Router<App> {
    router
        .route("/admin/login", post(login))
        .route("/admin/captcha/{id}", get(captcha_image))
        .route("/admin/devices/{id}/waiting", post(set_waiting))
        .route("/admin/devices/{id}/open", post(open_panel))
        .route("/admin/api/devices", get(device_list))
        .route("/admin/api/captcha", get(new_captcha))
        .route("/admin/devices/{id}/end", post(end_session))
        .route("/admin/devices/{id}/forget", post(forget))
        .route("/admin/devices/{id}/view", post(set_view))
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
