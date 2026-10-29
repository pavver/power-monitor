//! What the web page reads: setup stage, live status and visible networks.

use serde::Serialize;

use crate::message::format_duration;
use crate::power::PowerState;
use crate::settings::Messages;

/// Which screen the page shows, decided by what has been saved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Wifi,
    Bot,
    Dashboard,
}

impl Stage {
    /// The setup network stays up until the bot is set up: that step needs the
    /// home network, and until then the setup network is the way back in.
    pub fn needs_setup_ap(self) -> bool {
        self != Stage::Dashboard
    }
}

/// The setup access point while it is up, with the page's address on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SetupAp {
    pub ssid: &'static str,
    pub portal: String,
}

impl SetupAp {
    pub fn new(ssid: &'static str) -> Self {
        Self { ssid, portal: crate::portal::portal_url() }
    }
}

pub fn stage(wifi_saved: bool, bot_saved: bool) -> Stage {
    match (wifi_saved, bot_saved) {
        (false, _) => Stage::Wifi,
        (true, false) => Stage::Bot,
        (true, true) => Stage::Dashboard,
    }
}

/// The station connection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Link {
    #[default]
    Idle,
    Connecting { ssid: String },
    Connected { ssid: String, ip: String },
    Failed { ssid: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Network {
    pub ssid: String,
    /// Signal strength, dBm.
    pub rssi: i8,
    pub secure: bool,
}

/// Drops hidden networks and repeats of the same name, strongest first.
pub fn tidy_networks(mut networks: Vec<Network>) -> Vec<Network> {
    networks.retain(|n| !n.ssid.is_empty());
    networks.sort_by_key(|n| std::cmp::Reverse(n.rssi));
    let mut seen = Vec::new();
    networks.retain(|n| {
        let first = !seen.contains(&n.ssid);
        seen.push(n.ssid.clone());
        first
    });
    networks
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Power {
    pub present: bool,
    /// How long the current state has lasted, formatted here so the page
    /// does not repeat the duration rules; `None` when it began before
    /// start-up and its length is unknown.
    pub lasted: Option<String>,
}

impl Power {
    pub fn new(state: PowerState, lasted_ms: Option<u64>) -> Self {
        Self { present: state == PowerState::Present, lasted: lasted_ms.map(format_duration) }
    }
}

/// The running firmware.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Firmware {
    pub version: String,
    pub built: String,
    /// OTA slot it runs from, `ota_0` or `ota_1`.
    pub slot: String,
}

#[derive(Debug, Serialize)]
pub struct State<'a> {
    pub stage: Stage,
    /// `None` until the first state is confirmed.
    pub power: Option<Power>,
    pub link: &'a Link,
    /// The setup access point while it is up.
    pub setup_ap: Option<&'a SetupAp>,
    pub telegram: &'a str,
    pub pending: usize,
    pub chat_id: Option<i64>,
    /// The forum topic reports go to, if any.
    pub thread_id: Option<i64>,
    pub messages: &'a Messages,
    pub firmware: &'a Firmware,
    /// Whether the page asks for a password.
    pub password_set: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn net(ssid: &str, rssi: i8) -> Network {
        Network { ssid: ssid.to_string(), rssi, secure: true }
    }

    #[test]
    fn stage_follows_what_is_saved() {
        assert_eq!(stage(false, false), Stage::Wifi);
        assert_eq!(stage(false, true), Stage::Wifi);
        assert_eq!(stage(true, false), Stage::Bot);
        assert_eq!(stage(true, true), Stage::Dashboard);
    }

    #[test]
    fn setup_ap_runs_until_the_bot_is_set_up() {
        // The bot is set up over the home network, so the setup network must
        // stay reachable until then; afterwards it only exposes the panel.
        assert!(Stage::Wifi.needs_setup_ap());
        assert!(Stage::Bot.needs_setup_ap());
        assert!(!Stage::Dashboard.needs_setup_ap());
    }

    #[test]
    fn setup_ap_tells_the_page_its_portal_address() {
        let value = serde_json::to_value(SetupAp::new("PowerMonitor-Setup")).unwrap();
        assert_eq!(value, json!({"ssid": "PowerMonitor-Setup", "portal": "http://192.168.71.1/"}));
    }

    #[test]
    fn power_found_at_start_up_has_no_length() {
        let value = serde_json::to_value(Power::new(PowerState::Absent, None)).unwrap();
        assert_eq!(value, json!({"present": false, "lasted": null}));
    }

    #[test]
    fn networks_are_unique_visible_and_strongest_first() {
        let tidy = tidy_networks(vec![net("A", -80), net("", -30), net("B", -50), net("A", -40), net("C", -90)]);
        assert_eq!(tidy, vec![net("A", -40), net("B", -50), net("C", -90)]);
    }

    #[test]
    fn state_serializes_for_the_page() {
        let link = Link::Connected { ssid: "Home".to_string(), ip: "192.168.1.5".to_string() };
        let messages = Messages::default();
        let firmware = Firmware { version: "v1".into(), built: "Oct 29 2026".into(), slot: "ota_1".into() };
        let state = State {
            stage: Stage::Dashboard,
            power: Some(Power::new(PowerState::Present, Some(2 * 3_600_000 + 60_000))),
            link: &link,
            setup_ap: None,
            telegram: "ok",
            pending: 0,
            chat_id: Some(42),
            thread_id: Some(7),
            messages: &messages,
            firmware: &firmware,
            password_set: true,
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["stage"], json!("dashboard"));
        assert_eq!(value["power"], json!({"present": true, "lasted": "2 год 1 хв"}));
        assert_eq!(value["link"], json!({"state": "connected", "ssid": "Home", "ip": "192.168.1.5"}));
        assert_eq!(value["setup_ap"], json!(null));
        assert_eq!(value["messages"]["device_name"], json!("Дім"));
        assert_eq!(value["password_set"], json!(true));
        assert_eq!(value["thread_id"], json!(7));
        assert_eq!(value["firmware"], json!({"version": "v1", "built": "Oct 29 2026", "slot": "ota_1"}));
    }
}
