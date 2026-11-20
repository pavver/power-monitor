//! Telegram message types and duration formatting.

pub use autotext::template::UNKNOWN;

use crate::power::PowerEvent;

/// Human-readable duration in Ukrainian, at most two units: "2 год 15 хв".
pub fn format_duration(ms: u64) -> String {
    const UNITS: [(u64, &str); 4] =
        [(86_400_000, "д"), (3_600_000, "год"), (60_000, "хв"), (1_000, "с")];
    let major = UNITS.iter().position(|&(unit, _)| ms >= unit).unwrap_or(UNITS.len() - 1);
    let (unit, name) = UNITS[major];
    let text = format!("{} {name}", ms / unit);
    // Seconds only matter while nothing bigger is there to show.
    match UNITS.get(major + 1) {
        Some(&(minor, minor_name)) if minor > 1_000 && ms % unit >= minor => {
            format!("{text} {} {minor_name}", ms % unit / minor)
        }
        _ => text,
    }
}

/// Something to tell the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Power(PowerEvent),
    /// The device got a new address on the home network.
    Address(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const SEC: u64 = 1_000;
    const MIN: u64 = 60 * SEC;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;

    #[test]
    fn durations_use_the_two_largest_units() {
        assert_eq!(format_duration(0), "0 с");
        assert_eq!(format_duration(59 * SEC + 999), "59 с");
        assert_eq!(format_duration(MIN), "1 хв");
        assert_eq!(format_duration(59 * MIN + 59 * SEC), "59 хв");
        assert_eq!(format_duration(2 * HOUR + 15 * MIN + 30 * SEC), "2 год 15 хв");
        assert_eq!(format_duration(3 * HOUR), "3 год");
        assert_eq!(format_duration(DAY + 3 * HOUR + 20 * MIN), "1 д 3 год");
        assert_eq!(format_duration(2 * DAY), "2 д");
    }
}
