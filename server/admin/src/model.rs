//! A device as the server lists it, and how the list shows it.

use operator_ui::prelude::PresenceStatus;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceItem {
    pub id: String,
    pub name: String,
    pub version: String,
    pub seen: u64,
    pub online: bool,
    pub requested: bool,
    pub waiting: bool,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub view: Option<String>,
    #[serde(default)]
    pub pending_key: bool,
    #[serde(default)]
    pub pending_since: u64,
}

impl DeviceItem {
    pub fn display_name(&self) -> &str {
        let trimmed = self.name.trim();
        if trimmed.is_empty() {
            "без назви"
        } else {
            trimmed
        }
    }

    pub fn display_meta(&self) -> String {
        let seen = ago(self.seen);
        if self.version.is_empty() {
            format!("{} · {}", self.id, seen)
        } else {
            format!("{} · v{} · {}", self.id, self.version, seen)
        }
    }

    /// The badge for how the device is reachable.
    pub fn badge(&self) -> (PresenceStatus, &'static str) {
        if self.pending_key {
            return (PresenceStatus::Alarm, "новий ключ");
        }
        match self.status.as_str() {
            "session" => (PresenceStatus::Online, "з'єднано"),
            "alive" => (PresenceStatus::Online, "на зв'язку"),
            _ => (PresenceStatus::Offline, "не на зв'язку"),
        }
    }
}

/// Devices in a session first; otherwise in the server's order.
pub fn sort_devices(devices: &mut [DeviceItem]) {
    devices.sort_by_key(|d| d.status != "session");
}

pub fn ago(seconds: u64) -> String {
    if seconds < 60 {
        format!("{seconds} с тому")
    } else if seconds < 3600 {
        format!("{} хв тому", seconds / 60)
    } else if seconds < 86400 {
        format!("{} год тому", seconds / 3600)
    } else {
        format!("{} д тому", seconds / 86400)
    }
}

/// The device named by `?open=…`: the relay sends the admin back here with
/// it when the device's session is not running.
pub fn opened_device(search: &str) -> Option<String> {
    let query = search.strip_prefix('?').unwrap_or(search);
    query
        .split('&')
        .filter_map(|pair| pair.split_once('='))
        .find(|(key, value)| *key == "open" && !value.is_empty())
        .map(|(_, value)| value.to_string())
}

/// Whether the open panel switches to `server`, the screen the server shares,
/// given the shared screen seen before and the panel's own screen. Only a
/// change on the server is another admin moving: a local screen the server
/// does not share (an editor), or one not saved yet, is left alone.
pub fn follows(seen: Option<&str>, local: Option<&str>, server: &str) -> bool {
    seen != Some(server) && local != Some(server)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_follows_only_a_change_on_the_server() {
        // Another admin moved: follow.
        assert!(follows(Some("dashboard"), Some("dashboard"), "bms"));
        // The server still has what it had: stay, whatever the panel shows.
        assert!(!follows(Some("dashboard"), Some("text"), "dashboard"));
        // Our own move came back from the server: nothing to do.
        assert!(!follows(Some("dashboard"), Some("system"), "system"));
        // First look at a freshly opened panel that is elsewhere.
        assert!(follows(None, Some("login"), "wifi"));
    }

    fn device(id: &str, status: &str) -> DeviceItem {
        DeviceItem {
            id: id.into(),
            name: String::new(),
            version: String::new(),
            seen: 0,
            online: false,
            requested: false,
            waiting: false,
            status: status.into(),
            view: None,
            pending_key: false,
            pending_since: 0,
        }
    }

    #[test]
    fn ago_uses_the_largest_whole_unit() {
        assert_eq!(ago(59), "59 с тому");
        assert_eq!(ago(60), "1 хв тому");
        assert_eq!(ago(3599), "59 хв тому");
        assert_eq!(ago(3600), "1 год тому");
        assert_eq!(ago(86399), "23 год тому");
        assert_eq!(ago(86400), "1 д тому");
    }

    #[test]
    fn a_device_without_a_name_says_so() {
        let mut d = device("ab", "alive");
        assert_eq!(d.display_name(), "без назви");
        d.name = "  Дім ".into();
        assert_eq!(d.display_name(), "Дім");
    }

    #[test]
    fn meta_shows_the_version_when_known() {
        let mut d = device("ab", "alive");
        d.seen = 120;
        assert_eq!(d.display_meta(), "ab · 2 хв тому");
        d.version = "0.2.0".into();
        assert_eq!(d.display_meta(), "ab · v0.2.0 · 2 хв тому");
    }

    #[test]
    fn badges_follow_the_status() {
        assert_eq!(
            device("a", "session").badge(),
            (PresenceStatus::Online, "з'єднано")
        );
        assert_eq!(
            device("a", "alive").badge(),
            (PresenceStatus::Online, "на зв'язку")
        );
        assert_eq!(
            device("a", "offline").badge(),
            (PresenceStatus::Offline, "не на зв'язку")
        );
        let mut pending = device("a", "offline");
        pending.pending_key = true;
        assert_eq!(pending.badge(), (PresenceStatus::Alarm, "новий ключ"));
    }

    #[test]
    fn devices_in_a_session_come_first_and_the_rest_keep_their_order() {
        let mut list = [
            device("a", "alive"),
            device("b", "session"),
            device("c", "offline"),
            device("d", "session"),
        ];
        sort_devices(&mut list);
        let ids: Vec<&str> = list.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["b", "d", "a", "c"]);
    }

    #[test]
    fn the_device_to_open_comes_from_the_query() {
        assert_eq!(opened_device("?open=abc"), Some("abc".into()));
        assert_eq!(opened_device("?x=1&open=abc"), Some("abc".into()));
        assert_eq!(opened_device("open=abc"), Some("abc".into()));
        assert_eq!(opened_device("?open="), None);
        assert_eq!(opened_device("?opened=abc"), None);
        assert_eq!(opened_device(""), None);
    }
}
