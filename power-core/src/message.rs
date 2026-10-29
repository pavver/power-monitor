//! Telegram message texts.

use crate::power::{PowerEvent, PowerState};
use crate::settings::{Messages, Text};
use crate::template;

/// Events delivered later than this get a note saying when they happened,
/// since Wi-Fi is often down together with the power.
const LATE_AFTER_MS: u64 = 60_000;

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

/// The message for `event`, written at monotonic time `now_ms`.
pub fn event_text(messages: &Messages, event: &PowerEvent, now_ms: u64) -> String {
    let text = match (event.state, event.previous_lasted_ms) {
        (PowerState::Present, None) => Text::StartPresent,
        (PowerState::Absent, None) => Text::StartAbsent,
        (PowerState::Present, Some(_)) => Text::Returned,
        (PowerState::Absent, Some(_)) => Text::Lost,
    };
    let duration = format_duration(event.previous_lasted_ms.unwrap_or(0));
    let mut out = template::render(messages.template(text), messages.device_name(), &duration);
    let delay = now_ms.saturating_sub(event.at_ms);
    if delay >= LATE_AFTER_MS {
        out += &format!("\n(сталося {} тому)", format_duration(delay));
    }
    out
}

/// Sent from the web page to check the bot settings.
pub fn test_text(device: &str) -> String {
    format!("🔔 {device}: тестове повідомлення, бот працює.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form;
    use PowerState::*;

    const SEC: u64 = 1_000;
    const MIN: u64 = 60 * SEC;
    const HOUR: u64 = 60 * MIN;
    const DAY: u64 = 24 * HOUR;

    fn event(state: PowerState, previous_lasted_ms: Option<u64>) -> PowerEvent {
        PowerEvent { state, at_ms: 10 * SEC, previous_lasted_ms }
    }

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

    #[test]
    fn test_message_names_the_device() {
        assert_eq!(test_text("Дача"), "🔔 Дача: тестове повідомлення, бот працює.");
    }

    #[test]
    fn start_up_with_power() {
        assert_eq!(
            event_text(&Messages::default(), &event(Present, None), 10 * SEC),
            "🟢 Дім: монітор запущено, світло є."
        );
    }

    #[test]
    fn start_up_without_power() {
        assert_eq!(
            event_text(&Messages::default(), &event(Absent, None), 10 * SEC),
            "🔴 Дім: монітор запущено, світла немає."
        );
    }

    #[test]
    fn power_returned_says_how_long_it_was_out() {
        assert_eq!(
            event_text(&Messages::default(), &event(Present, Some(2 * HOUR + 15 * MIN)), 10 * SEC),
            "🟢 Дім: світло з'явилось.\nНе було 2 год 15 хв."
        );
    }

    #[test]
    fn power_lost_says_how_long_it_was_on() {
        assert_eq!(
            event_text(&Messages::default(), &event(Absent, Some(5 * MIN)), 10 * SEC),
            "🔴 Дім: світло зникло.\nБуло 5 хв."
        );
    }

    #[test]
    fn uses_the_users_templates() {
        let body = form::encode(&[
            ("device_name", "Дача"),
            ("start_present", "a"),
            ("start_absent", "b"),
            ("lost", "Нема світла на {device} після {duration}"),
            ("returned", "d"),
        ]);
        let messages = Messages::from_form(&body).unwrap();
        assert_eq!(
            event_text(&messages, &event(Absent, Some(3 * MIN)), 10 * SEC),
            "Нема світла на Дача після 3 хв"
        );
    }

    #[test]
    fn late_delivery_says_when_it_happened() {
        assert_eq!(
            event_text(&Messages::default(), &event(Absent, Some(5 * MIN)), 10 * SEC + 12 * MIN),
            "🔴 Дім: світло зникло.\nБуло 5 хв.\n(сталося 12 хв тому)"
        );
    }

    #[test]
    fn delivery_within_a_minute_is_not_late() {
        let text = event_text(&Messages::default(), &event(Present, None), 10 * SEC + LATE_AFTER_MS - 1);
        assert!(!text.contains("тому"), "{text}");
    }
}
