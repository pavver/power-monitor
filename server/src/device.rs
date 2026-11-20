//! Calls from the devices: check-ins, and in a session the long poll for
//! panel requests, their bodies and the panel's answers.

use std::sync::atomic::Ordering;

use axum::body::Bytes;
use axum::extract::{Form, Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use power_core::remote;
use serde::Deserialize;

use crate::relay::{split_head, Answer};
use crate::{App, POLL};

/// `router` with the device calls.
pub(crate) fn routes(router: Router<App>) -> Router<App> {
    router
        .route("/device/checkin", post(checkin))
        .route("/device/next", get(next))
        .route("/device/body/{rid}", get(body))
        .route("/device/reply/{rid}", post(reply))
}

#[derive(Deserialize)]
struct CheckInForm {
    #[serde(default)]
    name: String,
    #[serde(default)]
    version: String,
    /// "1" on the first check-in after the device started.
    #[serde(default)]
    start: String,
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
        // A device that has just started is waited for, so a session is soon at hand.
        device.waiting |= form.start == "1";
        serde_json::json!({ "session": device.want_session, "soon": device.waiting })
    };
    // Out of a session, so an end it never heard of is moot.
    app.link(&id).ending.store(false, Ordering::SeqCst);
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
    let started = app
        .0
        .devices
        .lock()
        .unwrap()
        .get_mut(&id)
        .is_some_and(|device| std::mem::take(&mut device.want_session));
    if started {
        app.save();
    }
    let deadline = tokio::time::Instant::now() + POLL;
    let mut inbox = link.inbox.lock().await;
    loop {
        let ended = link.ended.notified();
        tokio::pin!(ended);
        ended.as_mut().enable();
        if link.ending.swap(false, Ordering::SeqCst) {
            // Told to go: out of the session from now on.
            *link.last_poll.lock().unwrap() = None;
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
    let (sessions, headers) = panel_headers(&head);
    let link = app.link(&id);
    for value in &sessions {
        link.update_panel_session(value);
    }

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

/// The header lines of the panel's answer: the values of the panel session
/// cookie it sets, kept here and never shown to the browser, and the rest.
fn panel_headers(head: &str) -> (Vec<String>, Vec<(String, String)>) {
    let (sessions, rest): (Vec<_>, Vec<_>) = head
        .lines()
        .filter_map(|line| line.split_once(": "))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .partition(|(name, value)| {
            name.eq_ignore_ascii_case("set-cookie") && sets_panel_session(value)
        });
    (sessions.into_iter().map(|(_, value)| value).collect(), rest)
}

fn sets_panel_session(set_cookie: &str) -> bool {
    set_cookie
        .split(';')
        .next()
        .and_then(|pair| pair.split_once('='))
        .is_some_and(|(name, _)| name.trim() == remote::PANEL_COOKIE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_session_is_kept_apart_from_the_headers_for_the_browser() {
        let session = format!("{}=abc; Path=/; HttpOnly", remote::PANEL_COOKIE);
        let head = format!(
            "content-type: text/plain\nSet-Cookie: {session}\nset-cookie: theme=dark\nbroken line\n"
        );
        let (sessions, rest) = panel_headers(&head);
        assert_eq!(sessions, vec![session]);
        assert_eq!(
            rest,
            vec![
                ("content-type".to_string(), "text/plain".to_string()),
                ("set-cookie".to_string(), "theme=dark".to_string()),
            ]
        );
    }
}
