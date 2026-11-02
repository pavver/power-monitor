//! Telegram message texts.

use crate::jk::{self, Reading};
use crate::power::{Before, PowerEvent, PowerState};
use crate::settings::{Messages, Text};
use crate::template;

/// Power changes delivered later than this get a note saying when they
/// happened, since Wi-Fi is often down together with the power. Start-up
/// messages never do: they wait for Wi-Fi and the bot, so the delay is not
/// the time of any event.
const LATE_AFTER_MS: u64 = 60_000;

/// Stands for a length nobody measured, e.g. of the state found at start-up,
/// and for battery values when the BMS could not be read.
pub const UNKNOWN: &str = "🤷‍♂️";

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

/// The message for `event`, written at monotonic time `now_ms`; `bms` fills
/// the battery values (a shrug for each when there is no reading).
pub fn event_text(messages: &Messages, event: &PowerEvent, now_ms: u64, bms: Option<&Reading>) -> String {
    let start_up = event.before == Before::StartUp;
    let text = text_for(event);
    let duration = match event.before {
        Before::Lasted(ms) => format_duration(ms),
        _ => UNKNOWN.to_string(),
    };
    let battery = bms.map(Reading::values);
    let mut values = vec![("device", messages.device_name()), ("duration", duration.as_str())];
    match &battery {
        Some(battery) => values.extend(battery.iter().map(|(name, value)| (*name, value.as_str()))),
        None => values.extend(jk::VARIABLES.map(|name| (name, UNKNOWN))),
    }
    let mut out = template::render(messages.template(text), &values);
    let delay = now_ms.saturating_sub(event.at_ms);
    if !start_up && delay >= LATE_AFTER_MS {
        out += &format!("\n(сталося {} тому)", format_duration(delay));
    }
    out
}

fn text_for(event: &PowerEvent) -> Text {
    match (event.state, event.before == Before::StartUp) {
        (PowerState::Present, true) => Text::StartPresent,
        (PowerState::Absent, true) => Text::StartAbsent,
        (PowerState::Present, false) => Text::Returned,
        (PowerState::Absent, false) => Text::Lost,
    }
}

/// Whether the text for `notice` shows battery values, so the BMS has to be read first.
pub fn needs_bms(messages: &Messages, notice: &Notice) -> bool {
    match notice {
        Notice::Power(event) => template::uses_bms(messages.template(text_for(event))),
        Notice::Address(_) => false,
    }
}

/// Something to tell the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Power(PowerEvent),
    /// The device got a new address on the home network.
    Address(String),
}

/// What to send for `notice`; `None` when the user left its text empty.
pub fn notice_text(messages: &Messages, notice: &Notice, now_ms: u64, bms: Option<&Reading>) -> Option<String> {
    match notice {
        Notice::Power(event) if messages.template(text_for(event)).is_empty() => None,
        Notice::Power(event) => Some(event_text(messages, event, now_ms, bms)),
        Notice::Address(ip) => Some(format!("🌐 {}: нова адреса панелі http://{ip}/", messages.device_name())),
    }
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

    /// `None`: the start-up report; `Some(ms)`: a change after a state of known length.
    fn event(state: PowerState, previous_lasted_ms: Option<u64>) -> PowerEvent {
        let before = previous_lasted_ms.map_or(Before::StartUp, Before::Lasted);
        PowerEvent { state, at_ms: 10 * SEC, before }
    }

    fn change_after_start_up(state: PowerState) -> PowerEvent {
        PowerEvent { state, at_ms: 10 * SEC, before: Before::Unmeasured }
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
    fn new_address_is_sent_as_a_link() {
        let notice = Notice::Address("192.168.1.50".to_string());
        assert_eq!(
            notice_text(&Messages::default(), &notice, 0, None).as_deref(),
            Some("🌐 Дім: нова адреса панелі http://192.168.1.50/")
        );
    }

    #[test]
    fn a_blank_text_sends_nothing() {
        let body = form::encode(&[("device_name", "Дім"), ("start_present", "Є"), ("start_absent", " "), ("lost", ""), ("returned", "Є {duration}")]);
        let messages = Messages::from_form(&body).unwrap();
        let late = 10 * SEC + 30 * MIN;
        let send = |e: PowerEvent| notice_text(&messages, &Notice::Power(e), late, None);
        assert_eq!(send(event(Absent, Some(5 * MIN))), None);
        assert_eq!(send(event(Absent, None)), None);
        assert_eq!(send(event(Present, None)).as_deref(), Some("Є"));
        assert!(send(event(Present, Some(5 * MIN))).is_some());
    }

    #[test]
    fn power_notice_uses_the_event_text() {
        let e = event(Absent, Some(5 * MIN));
        assert_eq!(
            notice_text(&Messages::default(), &Notice::Power(e), 10 * SEC, None),
            Some(event_text(&Messages::default(), &e, 10 * SEC, None))
        );
    }

    #[test]
    fn test_message_names_the_device() {
        assert_eq!(test_text("Дача"), "🔔 Дача: тестове повідомлення, бот працює.");
    }

    #[test]
    fn start_up_with_power() {
        assert_eq!(
            event_text(&Messages::default(), &event(Present, None), 10 * SEC, None),
            "🟢 Дім: монітор запущено, світло є."
        );
    }

    #[test]
    fn start_up_without_power() {
        assert_eq!(
            event_text(&Messages::default(), &event(Absent, None), 10 * SEC, None),
            "🔴 Дім: монітор запущено, світла немає."
        );
    }

    #[test]
    fn power_returned_says_how_long_it_was_out() {
        assert_eq!(
            event_text(&Messages::default(), &event(Present, Some(2 * HOUR + 15 * MIN)), 10 * SEC, None),
            "🟢 Дім: світло з'явилось.\nНе було 2 год 15 хв."
        );
    }

    #[test]
    fn power_lost_says_how_long_it_was_on() {
        assert_eq!(
            event_text(&Messages::default(), &event(Absent, Some(5 * MIN)), 10 * SEC, None),
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
            event_text(&messages, &event(Absent, Some(3 * MIN)), 10 * SEC, None),
            "Нема світла на Дача після 3 хв"
        );
    }

    #[test]
    fn late_delivery_says_when_it_happened() {
        assert_eq!(
            event_text(&Messages::default(), &event(Absent, Some(5 * MIN)), 10 * SEC + 12 * MIN, None),
            "🔴 Дім: світло зникло.\nБуло 5 хв.\n(сталося 12 хв тому)"
        );
    }

    #[test]
    fn unknown_length_is_a_shrug() {
        // Power may have been off long before the device was switched on.
        assert_eq!(
            event_text(&Messages::default(), &change_after_start_up(Present), 10 * SEC, None),
            "🟢 Дім: світло з'явилось.\nНе було 🤷‍♂️."
        );
        assert_eq!(
            event_text(&Messages::default(), &change_after_start_up(Absent), 10 * SEC, None),
            "🔴 Дім: світло зникло.\nБуло 🤷‍♂️."
        );
    }

    #[test]
    fn unknown_length_in_a_user_template_is_a_shrug_too() {
        let body = form::encode(&[
            ("device_name", "Дача"),
            ("start_present", "a"),
            ("start_absent", "b"),
            ("lost", "Нема світла на {device} після {duration}"),
            ("returned", "d"),
        ]);
        let messages = Messages::from_form(&body).unwrap();
        assert_eq!(
            event_text(&messages, &change_after_start_up(Absent), 10 * SEC, None),
            "Нема світла на Дача після 🤷‍♂️"
        );
    }

    #[test]
    fn late_change_after_start_up_still_says_when_it_happened() {
        // The moment of the change is measured; only the length before it is not.
        let text = event_text(&Messages::default(), &change_after_start_up(Absent), 10 * SEC + 12 * MIN, None);
        assert_eq!(text, "🔴 Дім: світло зникло.\nБуло 🤷‍♂️.\n(сталося 12 хв тому)");
    }

    #[test]
    fn late_start_up_message_has_no_time_note() {
        // The start is reported once Wi-Fi and the bot work, often long after
        // boot; the delay says nothing about when anything happened.
        for state in [Present, Absent] {
            let text = event_text(&Messages::default(), &event(state, None), 10 * SEC + 30 * MIN, None);
            assert!(!text.contains("тому"), "{text}");
        }
    }

    fn battery_messages() -> Messages {
        let body = form::encode(&[
            ("device_name", "Дача"),
            ("start_present", "a"),
            ("start_absent", "b"),
            ("lost", "{device}: батарея {bms_soc}%, {bms_voltage} В"),
            ("returned", "d"),
        ]);
        Messages::from_form(&body).unwrap()
    }

    fn reading() -> Reading {
        Reading { voltage_mv: 13_280, soc: 84, remaining_mah: 84_000 }
    }

    #[test]
    fn battery_values_come_from_the_reading() {
        let text = event_text(&battery_messages(), &event(Absent, Some(MIN)), 10 * SEC, Some(&reading()));
        assert_eq!(text, "Дача: батарея 84%, 13.28 В");
    }

    #[test]
    fn missing_battery_reading_is_a_shrug() {
        let text = event_text(&battery_messages(), &event(Absent, Some(MIN)), 10 * SEC, None);
        assert_eq!(text, "Дача: батарея 🤷‍♂️%, 🤷‍♂️ В");
    }

    #[test]
    fn the_battery_is_read_only_for_texts_that_show_it() {
        let lost = Notice::Power(event(Absent, Some(MIN)));
        let returned = Notice::Power(event(Present, Some(MIN)));
        assert!(needs_bms(&battery_messages(), &lost));
        assert!(!needs_bms(&battery_messages(), &returned));
        assert!(!needs_bms(&battery_messages(), &Notice::Address("10.0.0.2".into())));
        assert!(!needs_bms(&Messages::default(), &lost));
    }

    #[test]
    fn delivery_within_a_minute_is_not_late() {
        let text = event_text(&Messages::default(), &event(Present, None), 10 * SEC + LATE_AFTER_MS - 1, None);
        assert!(!text.contains("тому"), "{text}");
    }
}
