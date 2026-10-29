//! Remote support (see power_core::remote): check-ins with the owner's server
//! and, in a session, requests to this panel relayed from there. The panel's
//! own web server answers them over loopback, so every page and API call,
//! firmware uploads included, works remotely exactly as it does at home.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use embedded_svc::http::client::Client;
use embedded_svc::http::Method as HttpMethod;
use embedded_svc::io::{Read, Write};
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use power_core::api::Link;
use power_core::remote::{self, Due, Identity, Method, Next, Phase, Schedule, Server, TunnelRequest};
use serde_json::json;

use crate::settings::Settings;
use crate::status::{now_ms, Status};

/// Longer than the server holds a long poll.
const SERVER_TIMEOUT: Duration = Duration::from_secs(40);
/// A firmware upload through loopback writes flash as it goes.
const LOCAL_TIMEOUT: Duration = Duration::from_secs(90);
const NO_WIFI_RETRY_MS: u64 = 60_000;
const FAILED_POLL_RETRY_MS: u64 = 5_000;
/// Polls failing in a row before the session is given up.
const FAILURES_TO_END: u32 = 3;
const MAX_ANSWER: usize = 4096;

type HttpClient = Client<EspHttpConnection>;

struct State {
    schedule: Schedule,
    error: Option<String>,
    last_contact_ms: Option<u64>,
}

#[derive(Clone)]
pub struct Remote {
    shared: Arc<(Mutex<State>, Condvar)>,
    settings: Arc<Settings>,
    status: Arc<Status>,
}

impl Remote {
    pub fn start(settings: Arc<Settings>, status: Arc<Status>) -> anyhow::Result<Self> {
        let schedule = Schedule::new(settings.remote().enabled, now_ms());
        let state = State { schedule, error: None, last_contact_ms: None };
        let remote = Self { shared: Arc::new((Mutex::new(state), Condvar::new())), settings, status };
        let worker = remote.clone();
        // TLS handshakes need a large stack.
        thread::Builder::new().stack_size(12 * 1024).spawn(move || worker.run())?;
        Ok(remote)
    }

    /// For the panel.
    pub fn view(&self) -> serde_json::Value {
        let settings = self.settings.remote();
        let now = now_ms();
        let state = self.lock();
        let (phase, next_in_s) = match state.schedule.phase() {
            Phase::Off => ("off", None),
            Phase::Waiting { at_ms } => ("waiting", Some(at_ms.saturating_sub(now) / 1000)),
            Phase::Session { .. } => ("session", None),
        };
        json!({
            "enabled": settings.enabled,
            "server": settings.server.as_ref().map(Server::as_str),
            "phase": phase,
            "next_in_s": next_in_s,
            "last_contact_s": state.last_contact_ms.map(|at| now.saturating_sub(at) / 1000),
            "error": state.error,
        })
    }

    /// The settings changed.
    pub fn reconfigure(&self) {
        let enabled = self.settings.remote().enabled;
        self.update(|state| {
            state.schedule.set_enabled(enabled, now_ms());
            state.error = None;
        });
    }

    pub fn connect_now(&self) {
        self.update(|state| state.schedule.connect_now(now_ms()));
    }

    pub fn end(&self) {
        self.update(|state| state.schedule.end(now_ms()));
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared.0.lock().unwrap()
    }

    fn update(&self, change: impl FnOnce(&mut State)) {
        change(&mut self.lock());
        self.shared.1.notify_all();
    }

    /// Sleeps for `ms` (or until woken); `None` sleeps until woken.
    fn sleep(&self, ms: Option<u64>) {
        let state = self.lock();
        match ms {
            Some(ms) => drop(self.shared.1.wait_timeout(state, Duration::from_millis(ms)).unwrap()),
            None => drop(self.shared.1.wait(state).unwrap()),
        }
    }

    fn record(&self, result: &Result<(), String>) {
        self.update(|state| match result {
            Ok(()) => {
                state.error = None;
                state.last_contact_ms = Some(now_ms());
            }
            Err(error) => state.error = Some(error.clone()),
        });
    }

    fn run(self) {
        // Kept for the whole session, so polls reuse one TLS connection.
        let mut session: Option<HttpClient> = None;
        let mut failures = 0;
        loop {
            let due = self.lock().schedule.due(now_ms());
            let server = self.settings.remote().active_server().cloned();
            let (Some(server), Due::CheckIn | Due::Poll) = (server, due) else {
                session = None;
                self.sleep(match due {
                    Due::Sleep { ms } => Some(ms),
                    _ => None,
                });
                continue;
            };
            if !matches!(self.status.link(), Link::Connected { .. }) {
                session = None;
                self.sleep(Some(NO_WIFI_RETRY_MS));
                continue;
            }
            let identity = match self.settings.identity() {
                Ok(identity) => identity,
                Err(error) => {
                    self.record(&Err(format!("не вдалося зберегти ідентифікатор: {error}")));
                    self.sleep(Some(NO_WIFI_RETRY_MS));
                    continue;
                }
            };
            if due == Due::CheckIn {
                let answer = self.check_in(&server, &identity);
                self.record(&answer.as_ref().map(|_| ()).map_err(Clone::clone));
                self.update(|state| state.schedule.checked_in(answer.map_err(|_| remote::RemoteError::BadAnswer), now_ms()));
                continue;
            }
            let client = match &mut session {
                Some(client) => client,
                None => match new_client(true) {
                    Ok(client) => session.insert(client),
                    Err(error) => {
                        self.record(&Err(error));
                        self.update(|state| state.schedule.end(now_ms()));
                        continue;
                    }
                },
            };
            match poll(client, &server, &identity) {
                Ok(next) => {
                    failures = 0;
                    self.record(&Ok(()));
                    match next {
                        Next::Wait => {}
                        Next::End => {
                            session = None;
                            self.update(|state| state.schedule.end(now_ms()));
                        }
                        Next::Request(request) => {
                            if let Err(error) = serve(client, &server, &identity, &request) {
                                log::warn!("remote {} {}: {error}", request.rid, request.path);
                                session = None;
                            }
                            self.update(|state| state.schedule.used(now_ms()));
                        }
                    }
                }
                Err(error) => {
                    log::warn!("remote poll: {error}");
                    session = None;
                    failures += 1;
                    self.record(&Err(error));
                    if failures >= FAILURES_TO_END {
                        failures = 0;
                        self.update(|state| state.schedule.end(now_ms()));
                    } else {
                        self.sleep(Some(FAILED_POLL_RETRY_MS));
                    }
                }
            }
        }
    }

    fn check_in(&self, server: &Server, identity: &Identity) -> Result<remote::CheckIn, String> {
        let mut client = new_client(true)?;
        let body = remote::checkin_form(self.settings.messages().device_name(), &crate::ota::running().version);
        let length = body.len().to_string();
        let mut headers = identity.headers().to_vec();
        headers.extend([("content-type", "application/x-www-form-urlencoded"), ("content-length", length.as_str())]);
        let url = server.url("/device/checkin");
        let (status, answer) = (|| {
            let mut request = client.post(&url, &headers)?;
            request.write_all(body.as_bytes())?;
            let mut response = request.submit()?;
            let status = response.status();
            anyhow::Ok((status, read_all(&mut response, MAX_ANSWER)?))
        })()
        .map_err(|e| format!("немає зв'язку з сервером ({e})"))?;
        remote::checkin_answer(status, &String::from_utf8_lossy(&answer)).map_err(|e| e.to_string())
    }
}

fn new_client(tls: bool) -> Result<HttpClient, String> {
    let connection = EspHttpConnection::new(&Configuration {
        crt_bundle_attach: tls.then_some(esp_idf_svc::sys::esp_crt_bundle_attach),
        timeout: Some(if tls { SERVER_TIMEOUT } else { LOCAL_TIMEOUT }),
        ..Default::default()
    })
    .map_err(|e| format!("HTTP-клієнт: {e}"))?;
    Ok(Client::wrap(connection))
}

fn poll(client: &mut HttpClient, server: &Server, identity: &Identity) -> Result<Next, String> {
    let (url, headers) = (server.url("/device/next"), identity.headers());
    let (status, answer) = (|| {
        let mut response = client.request(HttpMethod::Get, &url, &headers)?.submit()?;
        let status = response.status();
        anyhow::Ok((status, read_all(&mut response, MAX_ANSWER)?))
    })()
    .map_err(|e| format!("немає зв'язку з сервером ({e})"))?;
    remote::next_answer(status, &String::from_utf8_lossy(&answer)).map_err(|e| e.to_string())
}

/// Runs `request` against this panel and posts the answer back. A failure
/// on the panel side is still answered (502), so the browser is not left hanging.
fn serve(client: &mut HttpClient, server: &Server, identity: &Identity, request: &TunnelRequest) -> Result<(), String> {
    let (status, head, body) = local(client, server, identity, request).unwrap_or_else(|error| {
        log::warn!("remote {}: {error}", request.path);
        (502, "content-type: text/plain; charset=utf-8\n".to_string(), error.into_bytes())
    });
    let status = status.to_string();
    let length = (head.len() + 1 + body.len()).to_string();
    let mut headers = identity.headers().to_vec();
    headers.extend([("x-status", status.as_str()), ("content-length", length.as_str())]);
    let url = server.url(&format!("/device/reply/{}", request.rid));
    (|| {
        let mut post = client.post(&url, &headers)?;
        post.write_all(head.as_bytes())?;
        post.write_all(b"\n")?;
        post.write_all(&body)?;
        let mut response = post.submit()?;
        read_all(&mut response, MAX_ANSWER)?;
        anyhow::Ok(())
    })()
    .map_err(|e| format!("відповідь не надіслано ({e})"))
}

/// The panel's answer: status, browser headers, body. A request body is
/// streamed from the server straight into the panel.
fn local(
    client: &mut HttpClient,
    server: &Server,
    identity: &Identity,
    request: &TunnelRequest,
) -> Result<(u16, String, Vec<u8>), String> {
    let mut panel = new_client(false)?;
    let url = format!("http://127.0.0.1{}", request.path);
    let length = request.body_len.to_string();
    let mut headers = vec![("content-length", length.as_str())];
    if let Some(cookie) = &request.cookie {
        headers.push(("cookie", cookie));
    }
    if let Some(content_type) = &request.content_type {
        headers.push(("content-type", content_type));
    }
    let method = match request.method {
        Method::Get => HttpMethod::Get,
        Method::Post => HttpMethod::Post,
    };
    let body_url = server.url(&format!("/device/body/{}", request.rid));
    let identity_headers = identity.headers();
    (|| {
        let mut outgoing = panel.request(method, &url, &headers)?;
        if request.body_len > 0 {
            let mut incoming = client.request(HttpMethod::Get, &body_url, &identity_headers)?.submit()?;
            anyhow::ensure!(incoming.status() == 200, "тіло запиту недоступне ({})", incoming.status());
            let mut chunk = vec![0u8; 4096];
            let mut left = request.body_len;
            while left > 0 {
                let want = chunk.len().min(left);
                let read = incoming.read(&mut chunk[..want])?;
                anyhow::ensure!(read > 0, "тіло запиту обірвалось");
                outgoing.write_all(&chunk[..read])?;
                left -= read;
            }
        }
        let mut response = outgoing.submit()?;
        let status = response.status();
        let head = remote::reply_headers(|name| response.header(name).map(str::to_string));
        let body = read_all(&mut response, remote::MAX_REPLY)?;
        anyhow::Ok((status, head, body))
    })()
    .map_err(|e| format!("панель не відповіла ({e})"))
}

fn read_all(reader: &mut impl Read<Error = impl std::fmt::Debug>, limit: usize) -> anyhow::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = reader.read(&mut chunk).map_err(|e| anyhow::anyhow!("{e:?}"))?;
        if read == 0 {
            return Ok(bytes);
        }
        anyhow::ensure!(bytes.len() + read <= limit, "відповідь завелика");
        bytes.extend_from_slice(&chunk[..read]);
    }
}
