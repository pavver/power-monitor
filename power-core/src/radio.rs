//! Radio planning for the setup access point. The ESP32 has one radio: the
//! access point always sits on the station's channel, and a channel change
//! drops every phone on the setup network. So the access point starts on the
//! channel the station is most likely to join, and the station connects to a
//! known channel instead of sweeping all of them.

use std::time::Duration;

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

/// While the setup network is up the station joins only a network heard in
/// the last scan. A scan this old is repeated, so a router that starts after
/// the device (both powered back on together) is still found. Scans disturb
/// phones on the setup network, hence the long interval.
const RESCAN_UNHEARD_AFTER: Duration = Duration::from_secs(60);

/// Whether to scan again for a saved network the last scan did not hear,
/// given how long ago that scan was (`None`: never).
pub fn rescan_due(last_scan_age: Option<Duration>) -> bool {
    last_scan_age.is_none_or(|age| age >= RESCAN_UNHEARD_AFTER)
}

/// What to do with the setup network on a check of the link.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupApStep {
    Open,
    Close,
    Keep,
}

/// The setup network is up while the device has no home network to be
/// reached through, or while a phone is still on it. `held`: a join from the
/// page just succeeded; its channel change may have dropped the phone, which
/// comes back to read the result.
pub fn setup_ap_step(up: bool, linked: bool, phones: usize, held: bool) -> SetupApStep {
    match (up, linked) {
        (false, false) => SetupApStep::Open,
        (true, true) if phones == 0 && !held => SetupApStep::Close,
        _ => SetupApStep::Keep,
    }
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
    fn an_unheard_network_is_looked_for_again_once_the_scan_is_old() {
        assert!(rescan_due(None));
        assert!(!rescan_due(Some(Duration::from_secs(59))));
        assert!(rescan_due(Some(Duration::from_secs(60))));
    }

    #[test]
    fn setup_network_closes_once_linked_and_nobody_is_on_it() {
        assert_eq!(setup_ap_step(true, true, 0, false), SetupApStep::Close);
        assert_eq!(setup_ap_step(true, true, 1, false), SetupApStep::Keep);
        assert_eq!(setup_ap_step(true, false, 0, false), SetupApStep::Keep);
    }

    #[test]
    fn setup_network_waits_for_a_phone_that_dropped_off_while_joining() {
        // The channel change of a join drops phones; they come back to read the result.
        assert_eq!(setup_ap_step(true, true, 0, true), SetupApStep::Keep);
    }

    #[test]
    fn setup_network_opens_when_the_link_is_lost() {
        assert_eq!(setup_ap_step(false, false, 0, false), SetupApStep::Open);
        assert_eq!(setup_ap_step(false, true, 0, false), SetupApStep::Keep);
    }

    #[test]
    fn page_list_is_tidy() {
        let list = networks(&air());
        assert_eq!(list.iter().map(|n| n.ssid.as_str()).collect::<Vec<_>>(), ["Home", "Cafe"]);
    }
}
