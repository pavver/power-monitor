//! Global template values visible to all templates and automations.

use crate::clock;
use crate::jk;
use crate::power::PowerState;

/// All measured and system values at a point in time.
#[derive(Debug, Clone, Copy)]
pub struct Snapshot<'a> {
    pub device: &'a str,
    pub mains: Option<PowerState>,
    pub ip: Option<&'a str>,
    pub voltage_mv: Option<u32>,
    pub bms: Option<&'a jk::Reading>,
    pub local: Option<clock::Local>,
}

/// Template variable names in standard display order.
pub const NAMES: [&str; 11] = [
    "device",
    "mains",
    "ip",
    "battery_voltage",
    "bms_soc",
    "bms_voltage",
    "bms_energy",
    "bms_current",
    "bms_power",
    "time",
    "date",
];

/// Checks whether `name` is one of the known template variables.
pub fn known(name: &str) -> bool {
    NAMES.contains(&name)
}

/// Owned string representations of template variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Values {
    entries: [(&'static str, Option<String>); 11],
}

impl Values {
    /// Borrows values as a vector of pairs for autotext APIs.
    pub fn as_slice(&self) -> Vec<(&str, Option<&str>)> {
        self.entries.iter().map(|&(k, ref v)| (k, v.as_deref())).collect()
    }

    /// Looks up a variable value by name.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| *k == name)
            .and_then(|(_, v)| v.as_deref())
    }
}

/// Extracts template values from a measurement snapshot.
pub fn values(s: &Snapshot) -> Values {
    let mains_str = s.mains.map(|m| match m {
        PowerState::Present => "on".to_string(),
        PowerState::Absent => "off".to_string(),
    });

    let battery_voltage_str = s.voltage_mv.map(|mv| format!("{:.2}", f64::from(mv) / 1e3));
    let time_str = s.local.map(|l| l.time());
    let date_str = s.local.map(|l| l.date());

    let (soc, volt, energy, curr, power) = match s.bms {
        Some(bms) => {
            let [(_, s), (_, v), (_, e), (_, c), (_, p)] = bms.values();
            (Some(s), Some(v), Some(e), Some(c), Some(p))
        }
        None => (None, None, None, None, None),
    };

    Values {
        entries: [
            ("device", Some(s.device.to_string())),
            ("mains", mains_str),
            ("ip", s.ip.map(ToString::to_string)),
            ("battery_voltage", battery_voltage_str),
            ("bms_soc", soc),
            ("bms_voltage", volt),
            ("bms_energy", energy),
            ("bms_current", curr),
            ("bms_power", power),
            ("time", time_str),
            ("date", date_str),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_matches_names_list() {
        for name in NAMES {
            assert!(known(name));
        }
        assert!(!known("unknown_sensor"));
        assert!(!known(""));
    }

    #[test]
    fn values_with_full_measurements() {
        let reading = jk::Reading {
            voltage_mv: 52_400,
            soc: 85,
            remaining_mah: 100_000,
            current_ma: 15_000,
            power_mw: 786_000,
        };
        let local = clock::Local::new(2026, 11, 20, 14, 30);
        let snap = Snapshot {
            device: "Головний",
            mains: Some(PowerState::Present),
            ip: Some("192.168.1.55"),
            voltage_mv: Some(52_400),
            bms: Some(&reading),
            local: Some(local),
        };

        let v = values(&snap);
        assert_eq!(v.get("device"), Some("Головний"));
        assert_eq!(v.get("mains"), Some("on"));
        assert_eq!(v.get("ip"), Some("192.168.1.55"));
        assert_eq!(v.get("battery_voltage"), Some("52.40"));
        assert_eq!(v.get("bms_soc"), Some("85"));
        assert_eq!(v.get("bms_voltage"), Some("52.40"));
        assert_eq!(v.get("bms_energy"), Some("5.24"));
        assert_eq!(v.get("bms_current"), Some("15.0"));
        assert_eq!(v.get("bms_power"), Some("786"));
        assert_eq!(v.get("time"), Some("14:30"));
        assert_eq!(v.get("date"), Some("20.11.2026"));

        let slice = v.as_slice();
        assert_eq!(slice.len(), NAMES.len());
        for (i, name) in NAMES.iter().enumerate() {
            assert_eq!(slice[i].0, *name);
        }
    }

    #[test]
    fn values_with_no_measurements() {
        let snap = Snapshot {
            device: "Дім",
            mains: None,
            ip: None,
            voltage_mv: None,
            bms: None,
            local: None,
        };

        let v = values(&snap);
        assert_eq!(v.get("device"), Some("Дім"));
        assert_eq!(v.get("mains"), None);
        assert_eq!(v.get("ip"), None);
        assert_eq!(v.get("battery_voltage"), None);
        assert_eq!(v.get("bms_soc"), None);
        assert_eq!(v.get("bms_voltage"), None);
        assert_eq!(v.get("bms_energy"), None);
        assert_eq!(v.get("bms_current"), None);
        assert_eq!(v.get("bms_power"), None);
        assert_eq!(v.get("time"), None);
        assert_eq!(v.get("date"), None);
    }

    #[test]
    fn mains_power_state_mapping() {
        let snap_on = Snapshot {
            device: "Д",
            mains: Some(PowerState::Present),
            ip: None,
            voltage_mv: None,
            bms: None,
            local: None,
        };
        assert_eq!(values(&snap_on).get("mains"), Some("on"));

        let snap_off = Snapshot {
            device: "Д",
            mains: Some(PowerState::Absent),
            ip: None,
            voltage_mv: None,
            bms: None,
            local: None,
        };
        assert_eq!(values(&snap_off).get("mains"), Some("off"));
    }
}
