//! Radio planning for the setup access point. The ESP32 has one radio: the
//! access point always sits on the station's channel, and a channel change
//! drops every phone on the setup network. So the access point starts on the
//! channel the station is most likely to join, and the station connects to a
//! known channel instead of sweeping all of them.

use crate::api::{tidy_networks, Network};

/// One access point heard in a scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub ssid: String,
    pub rssi: i8,
    pub channel: u8,
    pub bssid: [u8; 6],
    pub secure: bool,
}

const FALLBACK_CHANNEL: u8 = 1;

/// The strongest access point named `ssid`.
pub fn strongest<'a>(seen: &'a [Seen], ssid: &str) -> Option<&'a Seen> {
    seen.iter().filter(|s| s.ssid == ssid).max_by_key(|s| s.rssi)
}

/// Channel for the setup access point: that of `preferred` when it is heard,
/// else of the strongest named network, else channel 1.
pub fn setup_channel(seen: &[Seen], preferred: Option<&str>) -> u8 {
    preferred
        .and_then(|ssid| strongest(seen, ssid))
        .or_else(|| seen.iter().filter(|s| !s.ssid.is_empty()).max_by_key(|s| s.rssi))
        .map_or(FALLBACK_CHANNEL, |s| s.channel)
}

/// The list the page shows.
pub fn networks(seen: &[Seen]) -> Vec<Network> {
    tidy_networks(
        seen.iter()
            .map(|s| Network { ssid: s.ssid.clone(), rssi: s.rssi, secure: s.secure })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ap(ssid: &str, rssi: i8, channel: u8) -> Seen {
        Seen { ssid: ssid.to_string(), rssi, channel, bssid: [channel; 6], secure: true }
    }

    fn air() -> Vec<Seen> {
        vec![ap("Home", -70, 6), ap("Home", -45, 11), ap("", -30, 3), ap("Cafe", -50, 1)]
    }

    #[test]
    fn strongest_picks_the_loudest_access_point_of_a_network() {
        assert_eq!(strongest(&air(), "Home"), Some(&ap("Home", -45, 11)));
        assert_eq!(strongest(&air(), "Absent"), None);
    }

    #[test]
    fn setup_channel_follows_the_preferred_network() {
        assert_eq!(setup_channel(&air(), Some("Cafe")), 1);
        assert_eq!(setup_channel(&air(), Some("Home")), 11);
    }

    #[test]
    fn setup_channel_falls_back_to_the_strongest_named_network() {
        // The hidden network at -30 dBm is nobody's choice.
        assert_eq!(setup_channel(&air(), None), 11);
        assert_eq!(setup_channel(&air(), Some("Absent")), 11);
    }

    #[test]
    fn setup_channel_on_empty_air_is_one() {
        assert_eq!(setup_channel(&[], None), 1);
    }

    #[test]
    fn page_list_is_tidy() {
        let list = networks(&air());
        assert_eq!(list.iter().map(|n| n.ssid.as_str()).collect::<Vec<_>>(), ["Home", "Cafe"]);
    }
}
