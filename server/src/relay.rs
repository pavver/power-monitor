//! Shared proxy per device: coalesced GETs (single-flight), 1.5 s GET cache,
//! and panel login session token kept per device on the server.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{to_bytes, Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use power_core::remote::{self, TunnelRequest};
use tokio::sync::{mpsc, oneshot, watch};

use crate::{cookie, random_hex, App, ANSWER, DEVICE_COOKIE, ONLINE, PICKUP};

/// How long a successful GET answer is kept in cache.
pub const CACHE_TTL: Duration = Duration::from_millis(1500);

/// Threshold for a device to be considered "alive" (checked in recently).
pub const ALIVE_LIMIT: Duration = Duration::from_secs(90 * 60);

/// Decides device status: session if polling, alive if seen within 90 minutes, offline otherwise.
pub fn device_status(online: bool, seen: Duration) -> &'static str {
    if online {
        "session"
    } else if seen <= ALIVE_LIMIT {
        "alive"
    } else {
        "offline"
    }
}

/// A browser request waiting on the device: told when the device takes it,
/// then given the answer. Dropping it tells the browser the session ended.
pub struct Pending {
    pub taken: Option<oneshot::Sender<()>>,
    pub answer: oneshot::Sender<Answer>,
    pub waiters: Arc<AtomicUsize>,
}

/// The device's answer: status, headers, body.
#[derive(Clone)]
pub struct Answer {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Bytes,
}

#[derive(Clone)]
pub enum RelayOutcome {
    Answer(Answer),
    Ended,
    Timeout,
}

pub struct Flight {
    pub rid: String,
    pub tx: watch::Sender<Option<RelayOutcome>>,
    pub rx: watch::Receiver<Option<RelayOutcome>>,
    pub waiters: Arc<AtomicUsize>,
}

pub struct WaiterGuard(pub Arc<AtomicUsize>);

impl Drop for WaiterGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One device's session plumbing.
pub struct Link {
    pub jobs: mpsc::UnboundedSender<TunnelRequest>,
    pub inbox: tokio::sync::Mutex<mpsc::UnboundedReceiver<TunnelRequest>>,
    pub bodies: Mutex<HashMap<String, Bytes>>,
    pub waiting: Mutex<HashMap<String, Pending>>,
    pub last_poll: Mutex<Option<Instant>>,
    /// The owner ended the session; the device's next poll learns it.
    pub ending: AtomicBool,
    /// Wakes a poll in progress when the session ends.
    pub ended: tokio::sync::Notify,
    pub panel_session: Mutex<Option<String>>,
    pub view: Mutex<Option<String>>,
    pub cache: Mutex<HashMap<String, (Answer, Instant)>>,
    pub flights: Mutex<HashMap<String, Arc<Flight>>>,
}

impl Default for Link {
    fn default() -> Self {
        Self::new()
    }
}

impl Link {
    pub fn new() -> Self {
        let (jobs, inbox) = mpsc::unbounded_channel();
        Self {
            jobs,
            inbox: tokio::sync::Mutex::new(inbox),
            bodies: Mutex::default(),
            waiting: Mutex::default(),
            last_poll: Mutex::default(),
            ending: AtomicBool::new(false),
            ended: tokio::sync::Notify::new(),
            panel_session: Mutex::default(),
            view: Mutex::default(),
            cache: Mutex::default(),
            flights: Mutex::default(),
        }
    }

    pub fn polled(&self) {
        *self.last_poll.lock().unwrap() = Some(Instant::now());
    }

    /// Ends the session: offline at once, every waiting browser let go, and
    /// the device told at its poll.
    pub fn end(&self) {
        *self.last_poll.lock().unwrap() = None;
        self.waiting.lock().unwrap().clear();
        self.bodies.lock().unwrap().clear();
        self.ending.store(true, Ordering::SeqCst);
        self.ended.notify_waiters();
        self.cache.lock().unwrap().clear();
        *self.view.lock().unwrap() = None;
        for (_, flight) in self.flights.lock().unwrap().drain() {
            let _ = flight.tx.send(Some(RelayOutcome::Ended));
        }
    }

    /// Marks a request taken by the device; `false` when no browser waits for it.
    pub fn take(&self, rid: &str) -> bool {
        let mut waiting = self.waiting.lock().unwrap();
        match waiting.get_mut(rid) {
            Some(pending) => {
                if pending.waiters.load(Ordering::SeqCst) == 0 {
                    return false;
                }
                if let Some(taken) = pending.taken.take() {
                    let _ = taken.send(());
                }
                true
            }
            None => false,
        }
    }

    pub fn online(&self) -> bool {
        self.last_poll
            .lock()
            .unwrap()
            .is_some_and(|at| at.elapsed() < ONLINE)
    }

    /// Updates stored panel session cookie from a Set-Cookie header value.
    pub fn update_panel_session(&self, set_cookie: &str) {
        let mut parts = set_cookie.split(';');
        let Some(first) = parts.next() else { return };
        let Some((name, val)) = first.split_once('=') else {
            return;
        };
        if name.trim() != remote::PANEL_COOKIE {
            return;
        }
        let val = val.trim();
        let max_age_zero = parts.any(|part| {
            part.trim()
                .split_once('=')
                .is_some_and(|(k, v)| k.trim().eq_ignore_ascii_case("max-age") && v.trim() == "0")
        });
        let mut session = self.panel_session.lock().unwrap();
        if val.is_empty() || max_age_zero {
            *session = None;
        } else {
            *session = Some(val.to_string());
        }
    }

    pub fn drop_cache(&self) {
        self.cache.lock().unwrap().clear();
    }

    pub fn set_view(&self, view: Option<String>) {
        *self.view.lock().unwrap() = view;
    }

    pub fn get_view(&self) -> Option<String> {
        self.view.lock().unwrap().clone()
    }
}

/// Converts an Answer into an axum Response.
pub fn answer_to_response(answer: Answer) -> Response {
    let mut response = Response::new(Body::from(answer.body));
    *response.status_mut() = StatusCode::from_u16(answer.status).unwrap_or(StatusCode::BAD_GATEWAY);
    for (name, value) in answer.headers {
        if let (Ok(name), Ok(value)) = (
            header::HeaderName::try_from(name),
            HeaderValue::try_from(value),
        ) {
            response.headers_mut().append(name, value);
        }
    }
    response
}

pub fn outcome_to_response(outcome: RelayOutcome, id: &str) -> Response {
    match outcome {
        RelayOutcome::Answer(answer) => answer_to_response(answer),
        RelayOutcome::Ended => Redirect::to(&format!("/admin?open={id}")).into_response(),
        RelayOutcome::Timeout => {
            (StatusCode::GATEWAY_TIMEOUT, "Пристрій не відповів.").into_response()
        }
    }
}

/// The header block and where the body starts.
pub fn split_head(body: &[u8]) -> Option<(String, usize)> {
    let end = if body.first() == Some(&b'\n') {
        0
    } else {
        body.windows(2).position(|w| w == b"\n\n")? + 1
    };
    Some((String::from_utf8_lossy(&body[..end]).into_owned(), end + 1))
}

async fn run_flight(link: Arc<Link>, path: String, flight: Arc<Flight>) {
    let rid = flight.rid.clone();
    let (taken, picked) = oneshot::channel();
    let (answer, answered) = oneshot::channel();

    link.waiting.lock().unwrap().insert(
        rid.clone(),
        Pending {
            taken: Some(taken),
            answer,
            waiters: Arc::clone(&flight.waiters),
        },
    );

    let cookie = link
        .panel_session
        .lock()
        .unwrap()
        .as_ref()
        .map(|token| format!("{}={token}", remote::PANEL_COOKIE));

    let job = TunnelRequest {
        rid: rid.clone(),
        method: remote::Method::Get,
        path: path.clone(),
        cookie,
        content_type: None,
        body_len: 0,
    };

    let _ = link.jobs.send(job);

    let result = match tokio::time::timeout(PICKUP, picked).await {
        Ok(Ok(())) => tokio::time::timeout(ANSWER, answered).await,
        Ok(Err(ended)) => Ok(Err(ended)),
        Err(busy) => Err(busy),
    };

    link.waiting.lock().unwrap().remove(&rid);

    let outcome = match result {
        Ok(Ok(answer)) => {
            // Do not cache a 504 / "device did not answer" result, and never cache when the session ended.
            if !link.ending.load(Ordering::SeqCst) && answer.status != 504 {
                link.cache
                    .lock()
                    .unwrap()
                    .insert(path.clone(), (answer.clone(), Instant::now()));
            }
            RelayOutcome::Answer(answer)
        }
        Ok(Err(_ended)) => RelayOutcome::Ended,
        Err(_) => RelayOutcome::Timeout,
    };

    link.flights.lock().unwrap().remove(&path);
    let _ = flight.tx.send(Some(outcome));
}

pub async fn relay(State(app): State<App>, request: Request) -> Response {
    let headers = request.headers().clone();
    if !app.is_admin(&headers) {
        return Redirect::to("/admin").into_response();
    }
    let Some(id) = cookie(&headers, DEVICE_COOKIE).map(str::to_string) else {
        return Redirect::to("/admin").into_response();
    };
    let link = app.link(&id);
    if !link.online() {
        return Redirect::to(&format!("/admin?open={id}")).into_response();
    }
    let method = match *request.method() {
        Method::GET => remote::Method::Get,
        Method::POST => remote::Method::Post,
        _ => return StatusCode::METHOD_NOT_ALLOWED.into_response(),
    };
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |p| p.as_str())
        .to_string();

    if method == remote::Method::Get {
        // 1. Check cache
        {
            let cache = link.cache.lock().unwrap();
            if let Some((answer, created_at)) = cache.get(&path) {
                if created_at.elapsed() < CACHE_TTL {
                    return answer_to_response(answer.clone());
                }
            }
        }

        // 2. Coalesce in-flight requests (single-flight)
        let (flight, is_leader) = {
            let mut flights = link.flights.lock().unwrap();
            if let Some(flight) = flights.get(&path) {
                flight.waiters.fetch_add(1, Ordering::SeqCst);
                (flight.clone(), false)
            } else {
                let (tx, rx) = watch::channel(None);
                let flight = Arc::new(Flight {
                    rid: random_hex(8),
                    tx,
                    rx,
                    waiters: Arc::new(AtomicUsize::new(1)),
                });
                flights.insert(path.clone(), flight.clone());
                (flight, true)
            }
        };

        if is_leader {
            let link_clone = Arc::clone(&link);
            let path_clone = path.clone();
            let flight_clone = Arc::clone(&flight);
            tokio::spawn(async move {
                run_flight(link_clone, path_clone, flight_clone).await;
            });
        }

        let _guard = WaiterGuard(Arc::clone(&flight.waiters));
        let mut rx = flight.rx.clone();
        loop {
            if let Some(outcome) = rx.borrow().clone() {
                return outcome_to_response(outcome, &id);
            }
            if rx.changed().await.is_err() {
                return (StatusCode::GATEWAY_TIMEOUT, "Пристрій не відповів.").into_response();
            }
        }
    }

    // POST request
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = match to_bytes(request.into_body(), remote::MAX_BODY).await {
        Ok(body) => body,
        Err(_) => return (StatusCode::PAYLOAD_TOO_LARGE, "Завеликий запит.").into_response(),
    };
    let rid = random_hex(8);
    let (taken, picked) = oneshot::channel();
    let (answer, answered) = oneshot::channel();
    let waiters = Arc::new(AtomicUsize::new(1));
    let _guard = WaiterGuard(Arc::clone(&waiters));

    link.waiting.lock().unwrap().insert(
        rid.clone(),
        Pending {
            taken: Some(taken),
            answer,
            waiters: Arc::clone(&waiters),
        },
    );
    if !body.is_empty() {
        link.bodies
            .lock()
            .unwrap()
            .insert(rid.clone(), body.clone());
    }

    let cookie = link
        .panel_session
        .lock()
        .unwrap()
        .as_ref()
        .map(|token| format!("{}={token}", remote::PANEL_COOKIE));

    let job = TunnelRequest {
        rid: rid.clone(),
        method: remote::Method::Post,
        path,
        cookie,
        content_type,
        body_len: body.len(),
    };
    let _ = link.jobs.send(job);

    let result = match tokio::time::timeout(PICKUP, picked).await {
        Ok(Ok(())) => tokio::time::timeout(ANSWER, answered).await,
        Ok(Err(ended)) => Ok(Err(ended)),
        Err(busy) => Err(busy),
    };

    link.waiting.lock().unwrap().remove(&rid);
    link.bodies.lock().unwrap().remove(&rid);

    // After any POST, drop that device's cache so the next read is fresh.
    link.drop_cache();

    match result {
        Ok(Ok(answer)) => answer_to_response(answer),
        Ok(Err(_ended)) => Redirect::to(&format!("/admin?open={id}")).into_response(),
        Err(_) => (StatusCode::GATEWAY_TIMEOUT, "Пристрій не відповів.").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_status_at_threshold() {
        let ninety_mins = Duration::from_secs(90 * 60);
        let ninety_mins_and_one_sec = ninety_mins + Duration::from_secs(1);

        // In a session, status is always "session"
        assert_eq!(device_status(true, Duration::ZERO), "session");
        assert_eq!(device_status(true, ninety_mins), "session");
        assert_eq!(device_status(true, ninety_mins_and_one_sec), "session");

        // Out of session: alive at most 90 minutes ago
        assert_eq!(device_status(false, Duration::ZERO), "alive");
        assert_eq!(device_status(false, ninety_mins), "alive");

        // Offline when longer than 90 minutes
        assert_eq!(device_status(false, ninety_mins_and_one_sec), "offline");
        assert_eq!(
            device_status(false, Duration::from_secs(24 * 3600)),
            "offline"
        );
    }
}
