//! Wall-clock time and local time zone.

use std::fmt;

pub const TIME: &str = "time";
pub const DATE: &str = "date";

/// Kyiv time zone POSIX string: EET (UTC+2), daylight saving EEST (UTC+3)
/// starting last Sunday in March at 03:00, ending last Sunday in October at 04:00.
pub const DEFAULT_TZ: &str = "EET-2EEST,M3.5.0/3,M10.5.0/4";

/// 2025-01-01 00:00:00 UTC unix timestamp in seconds.
pub const SET_THRESHOLD: i64 = 1_735_689_600;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClockError {
    InvalidTimeZone,
}

impl fmt::Display for ClockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTimeZone => f.write_str("часовий пояс у форматі POSIX, напр. EET-2EEST,M3.5.0/3,M10.5.0/4"),
        }
    }
}

impl std::error::Error for ClockError {}

/// A POSIX TZ string as libc's tzset takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimeZone(String);

impl Default for TimeZone {
    fn default() -> Self {
        Self(DEFAULT_TZ.to_string())
    }
}

impl TimeZone {
    /// Parses and validates a POSIX TZ string:
    /// trimmed, 1..=63 bytes, ASCII printable, starts with either 3+ ASCII letters
    /// or a `<...>` quoted name, followed by an offset that starts with a digit, '+' or '-'.
    pub fn parse(text: &str) -> Result<Self, ClockError> {
        let trimmed = text.trim();
        if !(1..=63).contains(&trimmed.len()) {
            return Err(ClockError::InvalidTimeZone);
        }
        if !trimmed.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
            return Err(ClockError::InvalidTimeZone);
        }
        let remainder = if trimmed.starts_with('<') {
            let close = trimmed.find('>').ok_or(ClockError::InvalidTimeZone)?;
            if close <= 1 {
                return Err(ClockError::InvalidTimeZone);
            }
            &trimmed[close + 1..]
        } else {
            let letters = trimmed.bytes().take_while(|b| b.is_ascii_alphabetic()).count();
            if letters < 3 {
                return Err(ClockError::InvalidTimeZone);
            }
            &trimmed[letters..]
        };
        match remainder.as_bytes().first() {
            Some(b) if b.is_ascii_digit() || *b == b'+' || *b == b'-' => {}
            _ => return Err(ClockError::InvalidTimeZone),
        }
        Ok(Self(trimmed.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for TimeZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::ops::Deref for TimeZone {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

/// Computes days since 1970-01-01 for a given Gregorian date.
pub fn days_since_1970(year: i32, month: u8, day: u8) -> u32 {
    let y = if month <= 2 { year - 1 } else { year } as i64;
    let m = if month <= 2 { month as i64 + 9 } else { month as i64 - 3 };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = (y - era * 400) as u32;
    let doy = (153 * m as u32 + 2) / 5 + day as u32 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe as i64 - 719468;
    days.max(0) as u32
}

/// Local time and date components built from libc `tm`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Local {
    pub year: i32,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub days: u32,
}

impl Local {
    pub fn new(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> Self {
        let days = days_since_1970(year, month, day);
        Self { year, month, day, hour, minute, days }
    }

    pub fn time(&self) -> String {
        format!("{:02}:{:02}", self.hour, self.minute)
    }

    pub fn date(&self) -> String {
        format!("{:02}.{:02}.{:04}", self.day, self.month, self.year)
    }

    pub fn minute_of_day(&self) -> u16 {
        self.hour as u16 * 60 + self.minute as u16
    }
}

/// The clock counts as set from 2025-01-01 on (before SNTP it starts at 1970).
pub fn is_set(unix_seconds: i64) -> bool {
    unix_seconds >= SET_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_time_zones() {
        assert_eq!(TimeZone::parse(DEFAULT_TZ).unwrap().as_str(), DEFAULT_TZ);
        assert_eq!(TimeZone::parse("UTC0").unwrap().as_str(), "UTC0");
        assert_eq!(TimeZone::parse("<+03>-3").unwrap().as_str(), "<+03>-3");
        assert_eq!(TimeZone::parse("  UTC0  ").unwrap().as_str(), "UTC0");
        assert_eq!(TimeZone::default().as_str(), DEFAULT_TZ);
    }

    #[test]
    fn parse_invalid_time_zones() {
        assert_eq!(TimeZone::parse(""), Err(ClockError::InvalidTimeZone));
        assert_eq!(TimeZone::parse("   "), Err(ClockError::InvalidTimeZone));
        assert_eq!(TimeZone::parse("Kyiv"), Err(ClockError::InvalidTimeZone));
        assert_eq!(TimeZone::parse("EET"), Err(ClockError::InvalidTimeZone));
        assert_eq!(TimeZone::parse(&"A".repeat(64)), Err(ClockError::InvalidTimeZone));
        assert_eq!(TimeZone::parse("EET-2Київ"), Err(ClockError::InvalidTimeZone));
        assert_eq!(TimeZone::parse("<>1"), Err(ClockError::InvalidTimeZone));
        assert_eq!(TimeZone::parse("ET5"), Err(ClockError::InvalidTimeZone));
        assert_eq!(TimeZone::parse("EET*1"), Err(ClockError::InvalidTimeZone));
    }

    #[test]
    fn clock_error_display() {
        assert_eq!(
            ClockError::InvalidTimeZone.to_string(),
            "часовий пояс у форматі POSIX, напр. EET-2EEST,M3.5.0/3,M10.5.0/4"
        );
    }

    #[test]
    fn formatting_time_and_date() {
        let local = Local {
            year: 2026,
            month: 11,
            day: 20,
            hour: 8,
            minute: 5,
            days: 20777,
        };
        assert_eq!(local.time(), "08:05");
        assert_eq!(local.date(), "20.11.2026");
        assert_eq!(local.minute_of_day(), 485);
    }

    #[test]
    fn minute_of_day_calculation() {
        assert_eq!(Local::new(2026, 1, 1, 0, 0).minute_of_day(), 0);
        assert_eq!(Local::new(2026, 1, 1, 23, 59).minute_of_day(), 1439);
        assert_eq!(Local::new(2026, 1, 1, 12, 34).minute_of_day(), 12 * 60 + 34);
    }

    #[test]
    fn days_since_1970_calculation() {
        assert_eq!(days_since_1970(1970, 1, 1), 0);
        assert_eq!(days_since_1970(1970, 1, 2), 1);
        assert_eq!(days_since_1970(2025, 1, 1), 20089);
        assert_eq!(days_since_1970(2026, 11, 20), 20777);
    }

    #[test]
    fn is_set_threshold() {
        assert!(!is_set(0));
        assert!(!is_set(SET_THRESHOLD - 1));
        assert!(is_set(SET_THRESHOLD));
        assert!(is_set(SET_THRESHOLD + 100));
    }
}
