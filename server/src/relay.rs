//! Shared proxy per device: coalesced GETs (single-flight), 1.5 s GET cache,
//! and panel login session token kept per device on the server.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::{to_bytes, Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode};
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

/// Recent answers to reads. Every change bumps `generation`, so an answer
/// asked for before a change is never kept after it.
#[derive(Default)]
pub struct Cache {
    generation: u64,
    answers: HashMap<String, (Answer, Instant)>,
}

impl Cache {
    fn fresh(&self, path: &str) -> Option<Answer> {
        let (answer, at) = self.answers.get(path)?;
        (at.elapsed() < CACHE_TTL).then(|| answer.clone())
    }

    /// Keeps `answer` to a read asked for in `generation`, if nothing changed since.
    fn keep(&mut self, generation: u64, path: &str, answer: &Answer) {
        if generation == self.generation {
            self.answers.retain(|_, (_, at)| at.elapsed() < CACHE_TTL);
            self.answers
                .insert(path.to_string(), (answer.clone(), Instant::now()));
        }
    }

    fn drop_all(&mut self) {
        self.answers.clear();
        self.generation += 1;
    }
}

pub struct Flight {
    pub rid: String,
    /// The cache generation when it was asked for.
    pub generation: u64,
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
    pub cache: Mutex<Cache>,
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
        self.drop_cache();
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
        self.cache.lock().unwrap().drop_all();
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

/// Hands `job` to the device and waits for its answer: first for the
/// device to take it, then for the answer. `body` waits for the device too.
/// The panel session kept here goes with it.
async fn ask(
    link: &Link,
    mut job: TunnelRequest,
    body: Bytes,
    waiters: Arc<AtomicUsize>,
) -> RelayOutcome {
    job.cookie = link
        .panel_session
        .lock()
        .unwrap()
        .as_ref()
        .map(|token| format!("{}={token}", remote::PANEL_COOKIE));
    let rid = job.rid.clone();
    let (taken, picked) = oneshot::channel();
    let (answer, answered) = oneshot::channel();
    let pending = Pending {
        taken: Some(taken),
        answer,
        waiters,
    };
    link.waiting.lock().unwrap().insert(rid.clone(), pending);
    if !body.is_empty() {
        link.bodies.lock().unwrap().insert(rid.clone(), body);
    }
    let _ = link.jobs.send(job);

    let result = match tokio::time::timeout(PICKUP, picked).await {
        Ok(Ok(())) => tokio::time::timeout(ANSWER, answered).await,
        Ok(Err(ended)) => Ok(Err(ended)),
        Err(busy) => Err(busy),
    };
    link.waiting.lock().unwrap().remove(&rid);
    link.bodies.lock().unwrap().remove(&rid);
    match result {
        Ok(Ok(answer)) => RelayOutcome::Answer(answer),
        Ok(Err(_ended)) => RelayOutcome::Ended,
        Err(_) => RelayOutcome::Timeout,
    }
}

async fn run_flight(link: Arc<Link>, path: String, flight: Arc<Flight>) {
    let job = TunnelRequest {
        rid: flight.rid.clone(),
        method: remote::Method::Get,
        path: path.clone(),
        cookie: None,
        content_type: None,
        body_len: 0,
    };
    let outcome = ask(&link, job, Bytes::new(), Arc::clone(&flight.waiters)).await;
    if let RelayOutcome::Answer(answer) = &outcome {
        // Not a 504 "device did not answer"; and nothing from before an end or a change.
        if answer.status != 504 {
            link.cache
                .lock()
                .unwrap()
                .keep(flight.generation, &path, answer);
        }
    }
    // Only its own: after an end a newer flight may have the path.
    let mut flights = link.flights.lock().unwrap();
    if flights.get(&path).is_some_and(|f| Arc::ptr_eq(f, &flight)) {
        flights.remove(&path);
    }
    drop(flights);
    let _ = flight.tx.send(Some(outcome));
}

/// The browser's request for the panel of the device the admin picked.
pub async fn relay(State(app): State<App>, request: Request) -> Response {
    let (id, link) = match picked(&app, request.headers()) {
        Ok(picked) => picked,
        Err(redirect) => return redirect.into_response(),
    };
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |p| p.as_str())
        .to_string();
    match *request.method() {
        Method::GET => get(link, path, &id).await,
        Method::POST => post(link, path, &id, request).await,
        _ => StatusCode::METHOD_NOT_ALLOWED.into_response(),
    }
}

/// The device picked by an admin and in a session, or where to go instead.
fn picked(app: &App, headers: &HeaderMap) -> Result<(String, Arc<Link>), Redirect> {
    let to_admin = || Redirect::to("/admin");
    if !app.is_admin(headers) {
        return Err(to_admin());
    }
    let id = cookie(headers, DEVICE_COOKIE)
        .ok_or_else(to_admin)?
        .to_string();
    let link = app.link(&id);
    if !link.online() {
        return Err(Redirect::to(&format!("/admin?open={id}")));
    }
    Ok((id, link))
}

/// A read: from the cache, or one request to the device shared by every
/// browser asking for the same path meanwhile.
async fn get(link: Arc<Link>, path: String, id: &str) -> Response {
    let cached = link.cache.lock().unwrap().fresh(&path);
    if let Some(answer) = cached {
        return answer_to_response(answer);
    }
    let flight = join_flight(&link, &path);
    let _guard = WaiterGuard(Arc::clone(&flight.waiters));
    let mut rx = flight.rx.clone();
    loop {
        if let Some(outcome) = rx.borrow().clone() {
            return outcome_to_response(outcome, id);
        }
        if rx.changed().await.is_err() {
            return outcome_to_response(RelayOutcome::Timeout, id);
        }
    }
}

/// Counts one more waiter, unless nobody waits any more.
fn join(waiters: &AtomicUsize) -> bool {
    let mut now = waiters.load(Ordering::SeqCst);
    while now > 0 {
        match waiters.compare_exchange_weak(now, now + 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return true,
            Err(changed) => now = changed,
        }
    }
    false
}

/// The request in flight for `path`, started if there is none; counts this
/// browser among its waiters. One nobody waits for any more is not joined:
/// the device may have skipped it already.
fn join_flight(link: &Arc<Link>, path: &str) -> Arc<Flight> {
    let mut flights = link.flights.lock().unwrap();
    if let Some(flight) = flights.get(path) {
        if join(&flight.waiters) {
            return flight.clone();
        }
    }
    let (tx, rx) = watch::channel(None);
    let flight = Arc::new(Flight {
        rid: random_hex(8),
        generation: link.cache.lock().unwrap().generation,
        tx,
        rx,
        waiters: Arc::new(AtomicUsize::new(1)),
    });
    flights.insert(path.to_string(), flight.clone());
    tokio::spawn(run_flight(
        Arc::clone(link),
        path.to_string(),
        Arc::clone(&flight),
    ));
    flight
}

/// A change: always its own request, and the cache is dropped after it.
async fn post(link: Arc<Link>, path: String, id: &str, request: Request) -> Response {
    let content_type = request
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let Ok(body) = to_bytes(request.into_body(), remote::MAX_BODY).await else {
        return (StatusCode::PAYLOAD_TOO_LARGE, "Завеликий запит.").into_response();
    };
    let waiters = Arc::new(AtomicUsize::new(1));
    let _guard = WaiterGuard(Arc::clone(&waiters));
    let job = TunnelRequest {
        rid: random_hex(8),
        method: remote::Method::Post,
        path,
        cookie: None,
        content_type,
        body_len: body.len(),
    };
    let outcome = ask(&link, job, body, waiters).await;
    // After any POST, drop that device's cache so the next read is fresh.
    link.drop_cache();
    outcome_to_response(outcome, id)
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
