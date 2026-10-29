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
    networks.sort_by(|a, b| b.rssi.cmp(&a.rssi));
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
    /// does not repeat the duration rules.
    pub lasted: String,
}

impl Power {
    pub fn new(state: PowerState, lasted_ms: u64) -> Self {
        Self { present: state == PowerState::Present, lasted: format_duration(lasted_ms) }
    }
}

#[derive(Debug, Serialize)]
pub struct State<'a> {
    pub stage: Stage,
    /// `None` until the first state is confirmed.
    pub power: Option<Power>,
    pub link: &'a Link,
    /// Name of the setup access point while it is up.
    pub setup_ap: Option<&'a str>,
    pub telegram: &'a str,
    pub pending: usize,
    pub chat_id: Option<i64>,
    pub messages: &'a Messages,
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
    fn networks_are_unique_visible_and_strongest_first() {
        let tidy = tidy_networks(vec![net("A", -80), net("", -30), net("B", -50), net("A", -40), net("C", -90)]);
        assert_eq!(tidy, vec![net("A", -40), net("B", -50), net("C", -90)]);
    }

    #[test]
    fn state_serializes_for_the_page() {
        let link = Link::Connected { ssid: "Home".to_string(), ip: "192.168.1.5".to_string() };
        let messages = Messages::default();
        let state = State {
            stage: Stage::Dashboard,
            power: Some(Power::new(PowerState::Present, 2 * 3_600_000 + 60_000)),
            link: &link,
            setup_ap: None,
            telegram: "ok",
            pending: 0,
            chat_id: Some(42),
            messages: &messages,
        };
        let value = serde_json::to_value(&state).unwrap();
        assert_eq!(value["stage"], json!("dashboard"));
        assert_eq!(value["power"], json!({"present": true, "lasted": "2 год 1 хв"}));
        assert_eq!(value["link"], json!({"state": "connected", "ssid": "Home", "ip": "192.168.1.5"}));
        assert_eq!(value["setup_ap"], json!(null));
        assert_eq!(value["messages"]["device_name"], json!("Дім"));
    }
}
