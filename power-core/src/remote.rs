//! Remote support. The device calls the owner's server, never the other way
//! round: once an hour it checks in, and when the server (or "connect now" on
//! the panel) asks for it, it opens a session: it long-polls the server for
//! requests to its own panel, runs each against its local web server and
//! posts the answer back. While the owner waits for the device (a flag on
//! the server) it checks in every two minutes instead. Everything here is
//! plain HTTPS.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::form;

pub const CHECKIN_EVERY_MS: u64 = 3_600_000;
/// The first check-in after start-up, so a fresh install shows up soon.
pub const FIRST_CHECKIN_MS: u64 = 60_000;
/// Check-ins while the owner waits for this device (the "waiting" flag on
/// the server), so a missed session is soon offered again.
pub const SOON_CHECKIN_MS: u64 = 2 * 60_000;
/// A session nobody uses closes after this.
pub const SESSION_IDLE_MS: u64 = 15 * 60_000;
/// Request bodies (a firmware image is the largest) and answers.
pub const MAX_BODY: usize = 2 * 1024 * 1024;
pub const MAX_REPLY: usize = 64 * 1024;
/// The panel's own session cookie; the only one passed through.
pub const PANEL_COOKIE: &str = "session";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteError {
    ServerUrl,
    /// The server refused this device (wrong secret, unknown, …).
    Refused(u16),
    BadAnswer,
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteError::ServerUrl => f.write_str("Адреса сервера має вигляд https://example.com"),
            RemoteError::Refused(403) => f.write_str("сервер не впізнав пристрій (403)"),
            RemoteError::Refused(status) => write!(f, "сервер відповів помилкою {status}"),
            RemoteError::BadAnswer => f.write_str("незрозуміла відповідь сервера"),
        }
    }
}

/// `https://host[:port][/base]`, kept without a trailing slash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Server(String);

impl Server {
    pub fn parse(text: &str) -> Result<Self, RemoteError> {
        let text = text.trim().trim_end_matches('/');
        let rest = text.strip_prefix("https://").ok_or(RemoteError::ServerUrl)?;
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        let host_ok = !host.is_empty() && host.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b));
        let path_ok = path.bytes().all(|b| b.is_ascii_graphic() && !b"?#".contains(&b));
        if !host_ok || !path_ok || text.len() > 100 {
            return Err(RemoteError::ServerUrl);
        }
        Ok(Self(text.to_string()))
    }

    /// `path` (starting with `/`) on this server.
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What the panel saves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteSettings {
    pub enabled: bool,
    pub server: Option<Server>,
}

impl RemoteSettings {
    /// Reads `enabled` (`1` when on) and `server`; switching on needs a server.
    pub fn from_form(body: &str) -> Result<Self, RemoteError> {
        let fields = form::parse(body);
        let enabled = form::field(&fields, "enabled") == "1";
        let server = match form::field(&fields, "server").trim() {
            "" => None,
            text => Some(Server::parse(text)?),
        };
        if enabled && server.is_none() {
            return Err(RemoteError::ServerUrl);
        }
        Ok(Self { enabled, server })
    }

    pub fn to_form(&self) -> String {
        let enabled = if self.enabled { "1" } else { "" };
        form::encode(&[("enabled", enabled), ("server", self.server.as_ref().map_or("", Server::as_str))])
    }

    /// The server to call, when remote support is on.
    pub fn active_server(&self) -> Option<&Server> {
        self.server.as_ref().filter(|_| self.enabled)
    }
}

/// Who the device is to the server. Made once from random bytes; the server
/// remembers the secret on first contact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub id: String,
    pub secret: String,
}

impl Identity {
    pub fn from_random(bytes: &[u8; 24]) -> Self {
        let hex = |bytes: &[u8]| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>();
        Self { id: hex(&bytes[..8]), secret: hex(&bytes[8..]) }
    }

    pub fn from_form(body: &str) -> Option<Self> {
        let fields = form::parse(body);
        let (id, secret) = (form::field(&fields, "id"), form::field(&fields, "secret"));
        let hex = |text: &str, len: usize| text.len() == len && text.bytes().all(|b| b.is_ascii_hexdigit());
        (hex(id, 16) && hex(secret, 32)).then(|| Self { id: id.to_string(), secret: secret.to_string() })
    }

    pub fn to_form(&self) -> String {
        form::encode(&[("id", &self.id), ("secret", &self.secret)])
    }

    /// Sent with every call to the server.
    pub fn headers(&self) -> [(&'static str, &str); 2] {
        [("x-device-id", &self.id), ("x-device-secret", &self.secret)]
    }
}

/// The check-in body: who and what this device is.
pub fn checkin_form(name: &str, version: &str) -> String {
    form::encode(&[("name", name), ("version", version)])
}

/// What the server wants after a check-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckIn {
    Idle,
    Session,
    /// The owner is waiting for this device: check in again soon.
    Soon,
}

pub fn checkin_answer(status: u16, body: &str) -> Result<CheckIn, RemoteError> {
    #[derive(Deserialize)]
    struct Answer {
        session: bool,
        #[serde(default)]
        soon: bool,
    }
    if status != 200 {
        return Err(RemoteError::Refused(status));
    }
    let answer: Answer = serde_json::from_str(body).map_err(|_| RemoteError::BadAnswer)?;
    Ok(match (answer.session, answer.soon) {
        (true, _) => CheckIn::Session,
        (false, true) => CheckIn::Soon,
        (false, false) => CheckIn::Idle,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Method {
    #[serde(rename = "GET")]
    Get,
    #[serde(rename = "POST")]
    Post,
}

/// A request to the panel, relayed by the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TunnelRequest {
    /// Names the request in the body and reply calls.
    pub rid: String,
    pub method: Method,
    pub path: String,
    #[serde(default)]
    pub cookie: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub body_len: usize,
}

/// The answer to a long poll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Nothing came in; ask again.
    Wait,
    /// The server closed the session.
    End,
    Request(TunnelRequest),
}

pub fn next_answer(status: u16, body: &str) -> Result<Next, RemoteError> {
    match status {
        204 => return Ok(Next::Wait),
        410 => return Ok(Next::End),
        200 => {}
        _ => return Err(RemoteError::Refused(status)),
    }
    let request: TunnelRequest = serde_json::from_str(body).map_err(|_| RemoteError::BadAnswer)?;
    let rid_ok = (1..=64).contains(&request.rid.len())
        && request.rid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    // Only a path on the panel itself, and nothing that could split headers.
    let path_ok = request.path.starts_with('/')
        && !request.path.starts_with("//")
        && request.path.bytes().all(|b| b.is_ascii_graphic());
    let headers_ok = [&request.cookie, &request.content_type].iter().all(|h| h.as_deref().is_none_or(header_safe));
    if !rid_ok || !path_ok || !headers_ok || request.body_len > MAX_BODY {
        return Err(RemoteError::BadAnswer);
    }
    Ok(Next::Request(request))
}

fn header_safe(value: &str) -> bool {
    !value.chars().any(char::is_control)
}

/// Response headers the browser needs.
const REPLY_HEADERS: [&str; 5] = ["content-type", "content-encoding", "set-cookie", "location", "cache-control"];

/// Response headers the browser needs, as `name: value` lines; anything else
/// stays on the device.
pub fn reply_headers(get: impl Fn(&str) -> Option<String>) -> String {
    REPLY_HEADERS
        .iter()
        .filter_map(|name| get(name).filter(|value| header_safe(value)).map(|value| format!("{name}: {value}\n")))
        .collect()
}

/// Where remote support stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Off,
    /// The next check-in is due at this monotonic time.
    Waiting { at_ms: u64 },
    /// Open; the last request (or the opening) was at this time.
    Session { last_ms: u64 },
}

/// What the remote thread should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// Nothing until woken.
    Never,
    Sleep { ms: u64 },
    CheckIn,
    Poll,
}

/// When to call the server. Monotonic milliseconds in, decisions out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Schedule {
    phase: Phase,
}

impl Schedule {
    pub fn new(enabled: bool, now_ms: u64) -> Self {
        let mut schedule = Self { phase: Phase::Off };
        schedule.set_enabled(enabled, now_ms);
        schedule
    }

    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn set_enabled(&mut self, enabled: bool, now_ms: u64) {
        self.phase = match (enabled, self.phase) {
            (false, _) => Phase::Off,
            (true, Phase::Off) => Phase::Waiting { at_ms: now_ms + FIRST_CHECKIN_MS },
            (true, running) => running,
        };
    }

    /// After a check-in (failed ones wait for the next hour too).
    pub fn checked_in(&mut self, answer: Result<CheckIn, RemoteError>, now_ms: u64) {
        if self.phase == Phase::Off {
            return;
        }
        self.phase = match answer {
            Ok(CheckIn::Session) => Phase::Session { last_ms: now_ms },
            Ok(CheckIn::Soon) => Phase::Waiting { at_ms: now_ms + SOON_CHECKIN_MS },
            Ok(CheckIn::Idle) | Err(_) => Phase::Waiting { at_ms: now_ms + CHECKIN_EVERY_MS },
        };
    }

    /// "Connect now" on the panel: a session without waiting for the hour.
    pub fn connect_now(&mut self, now_ms: u64) {
        if self.phase != Phase::Off {
            self.phase = Phase::Session { last_ms: now_ms };
        }
    }

    /// A request went through the session.
    pub fn used(&mut self, now_ms: u64) {
        if let Phase::Session { last_ms } = &mut self.phase {
            *last_ms = now_ms;
        }
    }

    /// The session closed (by the server, the panel, or a failure).
    pub fn end(&mut self, now_ms: u64) {
        if self.phase != Phase::Off {
            self.phase = Phase::Waiting { at_ms: now_ms + CHECKIN_EVERY_MS };
        }
    }

    /// What is due at `now_ms`; an idle session closes here.
    pub fn due(&mut self, now_ms: u64) -> Due {
        if let Phase::Session { last_ms } = self.phase {
            if now_ms >= last_ms + SESSION_IDLE_MS {
                self.end(now_ms);
            }
        }
        match self.phase {
            Phase::Off => Due::Never,
            Phase::Session { .. } => Due::Poll,
            Phase::Waiting { at_ms } if now_ms >= at_ms => Due::CheckIn,
            Phase::Waiting { at_ms } => Due::Sleep { ms: at_ms - now_ms },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: u64 = 60_000;
    const HOUR: u64 = 60 * MIN;

    #[test]
    fn server_must_be_https() {
        let server = Server::parse(" https://svitlo.example.com/ ").unwrap();
        assert_eq!(server.as_str(), "https://svitlo.example.com");
        assert_eq!(server.url("/device/checkin"), "https://svitlo.example.com/device/checkin");
        assert_eq!(Server::parse("https://example.com:8443/base").unwrap().url("/x"), "https://example.com:8443/base/x");
        for bad in ["", "example.com", "http://example.com", "https://", "https://exa mple.com", "https://a.com/?q=1", "https://a.com/#x"] {
            assert_eq!(Server::parse(bad), Err(RemoteError::ServerUrl), "{bad}");
        }
        assert_eq!(Server::parse(&format!("https://{}.com", "a".repeat(120))), Err(RemoteError::ServerUrl));
    }

    #[test]
    fn settings_round_trip() {
        let on = RemoteSettings::from_form("enabled=1&server=https%3A%2F%2Fa.example.com").unwrap();
        assert!(on.enabled);
        assert_eq!(on.active_server().map(Server::as_str), Some("https://a.example.com"));
        assert_eq!(RemoteSettings::from_form(&on.to_form()), Ok(on));
        let off = RemoteSettings::from_form("server=https%3A%2F%2Fa.example.com").unwrap();
        assert!(!off.enabled);
        assert_eq!(off.active_server(), None);
        assert_eq!(RemoteSettings::from_form("enabled=").unwrap(), RemoteSettings { enabled: false, server: None });
    }

    #[test]
    fn switching_on_needs_a_valid_server() {
        assert_eq!(RemoteSettings::from_form("enabled=1"), Err(RemoteError::ServerUrl));
        assert_eq!(RemoteSettings::from_form("enabled=1&server=ftp%3A%2F%2Fx"), Err(RemoteError::ServerUrl));
        // Off with a half-typed address is fine to keep only when empty.
        assert_eq!(RemoteSettings::from_form("server=nonsense"), Err(RemoteError::ServerUrl));
    }

    #[test]
    fn identity_is_hex_from_random_bytes() {
        let identity = Identity::from_random(&[0xAB; 24]);
        assert_eq!(identity.id, "abababababababab");
        assert_eq!(identity.secret, "ab".repeat(16));
        assert_eq!(Identity::from_form(&identity.to_form()), Some(identity));
        assert_eq!(Identity::from_form("id=&secret="), None);
    }

    #[test]
    fn checkin_answers() {
        assert_eq!(checkin_answer(200, r#"{"session":false}"#), Ok(CheckIn::Idle));
        assert_eq!(checkin_answer(200, r#"{"session":true}"#), Ok(CheckIn::Session));
        assert_eq!(checkin_answer(200, r#"{"session":false,"soon":true}"#), Ok(CheckIn::Soon));
        assert_eq!(checkin_answer(200, r#"{"session":true,"soon":true}"#), Ok(CheckIn::Session));
        assert_eq!(checkin_answer(403, "no"), Err(RemoteError::Refused(403)));
        assert_eq!(checkin_answer(200, "<html>"), Err(RemoteError::BadAnswer));
    }

    #[test]
    fn next_answers() {
        assert_eq!(next_answer(204, ""), Ok(Next::Wait));
        assert_eq!(next_answer(410, ""), Ok(Next::End));
        assert_eq!(next_answer(403, ""), Err(RemoteError::Refused(403)));
        let body = r#"{"rid":"r1","method":"POST","path":"/api/ota","cookie":"session=abc","content_type":"application/octet-stream","body_len":1700000}"#;
        let Ok(Next::Request(request)) = next_answer(200, body) else { panic!() };
        assert_eq!((request.rid.as_str(), request.method, request.path.as_str()), ("r1", Method::Post, "/api/ota"));
        assert_eq!(request.cookie.as_deref(), Some("session=abc"));
        assert_eq!(request.body_len, 1_700_000);
        let get = next_answer(200, r#"{"rid":"r2","method":"GET","path":"/"}"#).unwrap();
        assert!(matches!(get, Next::Request(TunnelRequest { body_len: 0, cookie: None, .. })));
    }

    #[test]
    fn relayed_requests_stay_on_the_panel() {
        let request = |path: &str, extra: &str| {
            next_answer(200, &format!(r#"{{"rid":"r","method":"GET","path":"{path}"{extra}}}"#))
        };
        for bad in ["api/state", "//evil.com/x", "http://evil.com/", r"/api/state\r\nX: y", "/a b"] {
            assert_eq!(request(bad, ""), Err(RemoteError::BadAnswer), "{bad}");
        }
        assert_eq!(request("/api/state", r#","cookie":"a\r\nb""#), Err(RemoteError::BadAnswer));
        assert_eq!(request("/api/ota", &format!(r#","body_len":{}"#, MAX_BODY + 1)), Err(RemoteError::BadAnswer));
        assert!(request("/api/x", r#","rid":"""#).is_err());
    }

    #[test]
    fn only_browser_headers_go_back() {
        let headers = reply_headers(|name| match name {
            "content-type" => Some("text/html".into()),
            "content-encoding" => Some("gzip".into()),
            "set-cookie" => Some("session=x; Path=/".into()),
            "location" => Some("bad\r\nheader".into()),
            "cache-control" => Some("no-store".into()),
            // Asked for nothing else: server, content-length, … stay behind.
            _ => Some("leak".into()),
        });
        assert_eq!(
            headers,
            "content-type: text/html\ncontent-encoding: gzip\nset-cookie: session=x; Path=/\ncache-control: no-store\n"
        );
    }

    #[test]
    fn first_check_in_comes_soon_after_start() {
        let mut schedule = Schedule::new(true, 0);
        assert_eq!(schedule.due(10_000), Due::Sleep { ms: FIRST_CHECKIN_MS - 10_000 });
        assert_eq!(schedule.due(FIRST_CHECKIN_MS), Due::CheckIn);
        assert_eq!(Schedule::new(false, 0).due(HOUR * 5), Due::Never);
    }

    #[test]
    fn checks_in_hourly_until_the_server_asks() {
        let mut schedule = Schedule::new(true, 0);
        schedule.checked_in(Ok(CheckIn::Idle), MIN);
        assert_eq!(schedule.due(MIN), Due::Sleep { ms: HOUR });
        schedule.checked_in(Err(RemoteError::BadAnswer), MIN + HOUR);
        assert_eq!(schedule.due(MIN + HOUR + 1), Due::Sleep { ms: HOUR - 1 });
        schedule.checked_in(Ok(CheckIn::Session), 2 * HOUR);
        assert_eq!(schedule.due(2 * HOUR + 1), Due::Poll);
    }

    #[test]
    fn a_waiting_owner_brings_check_ins_closer() {
        let mut schedule = Schedule::new(true, 0);
        schedule.checked_in(Ok(CheckIn::Soon), MIN);
        assert_eq!(schedule.due(MIN), Due::Sleep { ms: SOON_CHECKIN_MS });
        schedule.checked_in(Ok(CheckIn::Idle), MIN + SOON_CHECKIN_MS);
        assert_eq!(schedule.due(MIN + SOON_CHECKIN_MS), Due::Sleep { ms: HOUR });
    }

    #[test]
    fn idle_session_closes_and_use_keeps_it() {
        let mut schedule = Schedule::new(true, 0);
        schedule.connect_now(5 * MIN);
        assert_eq!(schedule.phase(), Phase::Session { last_ms: 5 * MIN });
        schedule.used(10 * MIN);
        assert_eq!(schedule.due(10 * MIN + SESSION_IDLE_MS - 1), Due::Poll);
        assert_eq!(schedule.due(10 * MIN + SESSION_IDLE_MS), Due::Sleep { ms: HOUR });
        assert_eq!(schedule.phase(), Phase::Waiting { at_ms: 10 * MIN + SESSION_IDLE_MS + HOUR });
    }

    #[test]
    fn ending_a_session_waits_an_hour() {
        let mut schedule = Schedule::new(true, 0);
        schedule.connect_now(MIN);
        schedule.end(2 * MIN);
        assert_eq!(schedule.phase(), Phase::Waiting { at_ms: 2 * MIN + HOUR });
    }

    #[test]
    fn switching_off_stops_everything_and_on_checks_in_soon() {
        let mut schedule = Schedule::new(true, 0);
        schedule.connect_now(MIN);
        schedule.set_enabled(false, 2 * MIN);
        assert_eq!(schedule.phase(), Phase::Off);
        schedule.connect_now(3 * MIN);
        assert_eq!(schedule.phase(), Phase::Off, "connect now does nothing while off");
        schedule.set_enabled(true, 4 * MIN);
        assert_eq!(schedule.phase(), Phase::Waiting { at_ms: 4 * MIN + FIRST_CHECKIN_MS });
        // Saving the settings again leaves a running schedule alone.
        schedule.set_enabled(true, 5 * MIN);
        assert_eq!(schedule.phase(), Phase::Waiting { at_ms: 4 * MIN + FIRST_CHECKIN_MS });
    }
}
