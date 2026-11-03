//! Telegram message texts.

use crate::form;
use crate::jk::{self, Reading};
use crate::power::{Before, PowerEvent, PowerState};
use crate::settings::{Messages, Text};
use crate::template;

/// Power changes delivered later than this have `{delay}`, how long ago they
/// happened, since Wi-Fi is often down together with the power. Start-up
/// messages never do: they wait for Wi-Fi and the bot, so the delay is not
/// the time of any event.
const LATE_AFTER_MS: u64 = 60_000;

pub use crate::template::UNKNOWN;

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

/// What was measured for a message; a value that is missing shows as a shrug.
#[derive(Debug, Clone, Copy)]
pub struct Readings<'a> {
    pub bms: Option<&'a Reading>,
    /// Battery voltage through the divider.
    pub voltage_mv: Option<u32>,
}

impl Readings<'_> {
    pub const NONE: Self = Readings { bms: None, voltage_mv: None };
}

/// The message for `event`, written at monotonic time `now_ms`.
pub fn event_text(messages: &Messages, event: &PowerEvent, now_ms: u64, readings: Readings) -> String {
    let duration = match event.before {
        Before::Lasted(ms) => Some(format_duration(ms)),
        _ => None,
    };
    let delay = now_ms.saturating_sub(event.at_ms);
    let late = event.before != Before::StartUp && delay >= LATE_AFTER_MS;
    let delay = late.then(|| format_duration(delay));
    let own = [("duration", duration.as_deref()), ("delay", delay.as_deref())];
    fill(messages.template(text_for(event)), messages.device_name(), &own, readings)
}

/// Stands in for `{duration}` in a preview, where nothing was measured.
const SAMPLE_DURATION_MS: u64 = 2 * 3_600_000 + 15 * 60_000;

/// Stands in for `{ip}` in a preview.
const SAMPLE_IP: &str = "192.168.1.50";

/// What the page shows while a text is edited: the unsaved `template` for
/// the `text` named in the form, with `device_name`, a sample length and the
/// values measured now; or why the template would not be saved.
pub fn preview(body: &str, readings: Readings) -> Result<String, String> {
    let fields = form::parse(body);
    let key = form::field(&fields, "text");
    let text = Text::ALL.into_iter().find(|text| text.key() == key).ok_or("невідомий текст")?;
    let template = form::field(&fields, "template");
    template::validate(template, text.own()).map_err(|error| error.to_string())?;
    // An on-time message: the usual case, so no `{delay}`.
    let duration = format_duration(SAMPLE_DURATION_MS);
    let own = [("duration", Some(duration.as_str())), ("delay", None), ("ip", Some(SAMPLE_IP))];
    Ok(fill(template, form::field(&fields, "device_name").trim(), &own, readings))
}

/// `template` with every value put in; a `None` one was not measured.
fn fill(template: &str, device: &str, own: &[(&str, Option<&str>)], readings: Readings) -> String {
    let voltage = readings.voltage_mv.map(|mv| format!("{:.2}", f64::from(mv) / 1e3));
    let battery = readings.bms.map(Reading::values);
    let mut values = vec![("device", Some(device)), (template::BATTERY_VOLTAGE, voltage.as_deref())];
    values.extend_from_slice(own);
    match &battery {
        Some(battery) => values.extend(battery.iter().map(|(name, value)| (*name, Some(value.as_str())))),
        None => values.extend(jk::VARIABLES.map(|name| (name, None))),
    }
    template::render(template, &values)
}

fn text_for(event: &PowerEvent) -> Text {
    match (event.state, event.before == Before::StartUp) {
        (PowerState::Present, true) => Text::StartPresent,
        (PowerState::Absent, true) => Text::StartAbsent,
        (PowerState::Present, false) => Text::Returned,
        (PowerState::Absent, false) => Text::Lost,
    }
}

fn text_for_notice(notice: &Notice) -> Text {
    match notice {
        Notice::Power(event) => text_for(event),
        Notice::Address(_) => Text::Address,
    }
}

/// Whether the text for `notice` shows battery values, so the BMS has to be read first.
pub fn needs_bms(messages: &Messages, notice: &Notice) -> bool {
    template::uses_bms(messages.template(text_for_notice(notice)))
}

/// Something to tell the chat.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
    Power(PowerEvent),
    /// The device got a new address on the home network.
    Address(String),
}

/// What to send for `notice`; `None` when the user left its text empty.
pub fn notice_text(messages: &Messages, notice: &Notice, now_ms: u64, readings: Readings) -> Option<String> {
    if messages.template(text_for_notice(notice)).is_empty() {
        return None;
    }
    Some(match notice {
        Notice::Power(event) => event_text(messages, event, now_ms, readings),
        Notice::Address(ip) => fill(messages.template(Text::Address), messages.device_name(), &[("ip", Some(ip))], readings),
    })
}

/// Sent from the web page to check the bot settings.
pub fn test_text(messages: &Messages, readings: Readings) -> String {
    fill(messages.template(Text::Test), messages.device_name(), &[], readings)
}

#[cfg(test)]
mod tests {
    use super::*;
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
            notice_text(&Messages::default(), &notice, 0, Readings::NONE).as_deref(),
            Some("🌐 Дім: нова адреса панелі http://192.168.1.50/")
        );
    }

    #[test]
    fn a_blank_text_sends_nothing() {
        let body = form_with(&[("device_name", "Дім"), ("start_present", "Є"), ("start_absent", " "), ("lost", ""), ("returned", "Є {duration}")]);
        let messages = Messages::from_form(&body).unwrap();
        let late = 10 * SEC + 30 * MIN;
        let send = |e: PowerEvent| notice_text(&messages, &Notice::Power(e), late, Readings::NONE);
        assert_eq!(send(event(Absent, Some(5 * MIN))), None);
        assert_eq!(send(event(Absent, None)), None);
        assert_eq!(send(event(Present, None)).as_deref(), Some("Є"));
        assert!(send(event(Present, Some(5 * MIN))).is_some());
    }

    #[test]
    fn power_notice_uses_the_event_text() {
        let e = event(Absent, Some(5 * MIN));
        assert_eq!(
            notice_text(&Messages::default(), &Notice::Power(e), 10 * SEC, Readings::NONE),
            Some(event_text(&Messages::default(), &e, 10 * SEC, Readings::NONE))
        );
    }

    #[test]
    fn test_message_names_the_device() {
        let messages = Messages::from_form(&form_with(&[("device_name", "Дача")])).unwrap();
        assert_eq!(test_text(&messages, Readings::NONE), "🔔 Дача: тестове повідомлення, бот працює.");
    }

    #[test]
    fn test_message_shows_the_battery() {
        let divider = Readings { voltage_mv: Some(52_020), ..Readings::NONE };
        assert_eq!(test_text(&Messages::default(), divider), "🔔 Дім: тестове повідомлення, бот працює.\n🔋 52.02 В");
    }

    #[test]
    fn test_message_uses_its_template() {
        let messages = Messages::from_form(&form_with(&[("test", "Перевірка {device}")])).unwrap();
        assert_eq!(test_text(&messages, Readings::NONE), "Перевірка Дім");
    }

    /// The default texts as the page would send them, some replaced.
    fn form_with(changes: &[(&str, &str)]) -> String {
        let defaults = Messages::default();
        let mut fields = defaults.fields();
        for &(key, value) in changes {
            fields.iter_mut().find(|(k, _)| *k == key).unwrap().1 = value;
        }
        form::encode(&fields)
    }

    #[test]
    fn address_message_uses_its_template() {
        let messages = Messages::from_form(&form_with(&[("address", "{device} тепер на {ip}")])).unwrap();
        let notice = Notice::Address("10.0.0.2".to_string());
        assert_eq!(notice_text(&messages, &notice, 0, Readings::NONE).as_deref(), Some("Дім тепер на 10.0.0.2"));
        let off = Messages::from_form(&form_with(&[("address", "")])).unwrap();
        assert_eq!(notice_text(&off, &notice, 0, Readings::NONE), None);
    }

    #[test]
    fn late_note_is_where_the_template_puts_it() {
        let messages = Messages::from_form(&form_with(&[("lost", "Зникло{delay? ({delay} тому)}.")])).unwrap();
        let late = event_text(&messages, &event(Absent, Some(5 * MIN)), 10 * SEC + 12 * MIN, Readings::NONE);
        assert_eq!(late, "Зникло (12 хв тому).");
        let on_time = event_text(&messages, &event(Absent, Some(5 * MIN)), 10 * SEC, Readings::NONE);
        assert_eq!(on_time, "Зникло.");
    }

    #[test]
    fn a_text_without_delay_has_no_late_note() {
        let messages = Messages::from_form(&form_with(&[("lost", "Зникло")])).unwrap();
        let late = event_text(&messages, &event(Absent, Some(5 * MIN)), 10 * SEC + 12 * MIN, Readings::NONE);
        assert_eq!(late, "Зникло");
    }

    #[test]
    fn preview_of_the_address_fills_a_sample_one() {
        let body = preview_form("address", "{device}: http://{ip}/");
        assert_eq!(preview(&body, Readings::NONE), Ok("Дача: http://192.168.1.50/".to_string()));
    }

    #[test]
    fn start_up_with_power() {
        assert_eq!(
            event_text(&Messages::default(), &event(Present, None), 10 * SEC, Readings::NONE),
            "⚡ Дім: монітор запущено, світло є."
        );
    }

    #[test]
    fn start_up_without_power() {
        assert_eq!(
            event_text(&Messages::default(), &event(Absent, None), 10 * SEC, Readings::NONE),
            "🔌 Дім: монітор запущено, світла немає."
        );
    }

    #[test]
    fn power_returned_says_how_long_it_was_out() {
        assert_eq!(
            event_text(&Messages::default(), &event(Present, Some(2 * HOUR + 15 * MIN)), 10 * SEC, Readings::NONE),
            "⚡ Дім: світло з'явилось.\nНе було 2 год 15 хв."
        );
    }

    #[test]
    fn power_lost_says_how_long_it_was_on() {
        assert_eq!(
            event_text(&Messages::default(), &event(Absent, Some(5 * MIN)), 10 * SEC, Readings::NONE),
            "🔌 Дім: світло зникло.\nБуло 5 хв."
        );
    }

    #[test]
    fn uses_the_users_templates() {
        let body = form_with(&[
            ("device_name", "Дача"),
            ("start_present", "a"),
            ("start_absent", "b"),
            ("lost", "Нема світла на {device} після {duration}"),
            ("returned", "d"),
        ]);
        let messages = Messages::from_form(&body).unwrap();
        assert_eq!(
            event_text(&messages, &event(Absent, Some(3 * MIN)), 10 * SEC, Readings::NONE),
            "Нема світла на Дача після 3 хв"
        );
    }

    #[test]
    fn late_delivery_says_when_it_happened() {
        assert_eq!(
            event_text(&Messages::default(), &event(Absent, Some(5 * MIN)), 10 * SEC + 12 * MIN, Readings::NONE),
            "🔌 Дім: світло зникло.\nБуло 5 хв.\n(сталося 12 хв тому)"
        );
    }

    #[test]
    fn default_texts_leave_out_an_unknown_length() {
        // Power may have been off long before the device was switched on.
        assert_eq!(
            event_text(&Messages::default(), &change_after_start_up(Present), 10 * SEC, Readings::NONE),
            "⚡ Дім: світло з'явилось."
        );
        assert_eq!(
            event_text(&Messages::default(), &change_after_start_up(Absent), 10 * SEC, Readings::NONE),
            "🔌 Дім: світло зникло."
        );
    }

    #[test]
    fn unknown_length_in_a_user_template_is_a_shrug_too() {
        let body = form_with(&[
            ("device_name", "Дача"),
            ("start_present", "a"),
            ("start_absent", "b"),
            ("lost", "Нема світла на {device} після {duration}"),
            ("returned", "d"),
        ]);
        let messages = Messages::from_form(&body).unwrap();
        assert_eq!(
            event_text(&messages, &change_after_start_up(Absent), 10 * SEC, Readings::NONE),
            "Нема світла на Дача після 🤷‍♂️"
        );
    }

    #[test]
    fn late_change_after_start_up_still_says_when_it_happened() {
        // The moment of the change is measured; only the length before it is not.
        let text = event_text(&Messages::default(), &change_after_start_up(Absent), 10 * SEC + 12 * MIN, Readings::NONE);
        assert_eq!(text, "🔌 Дім: світло зникло.\n(сталося 12 хв тому)");
    }

    #[test]
    fn late_start_up_message_has_no_time_note() {
        // The start is reported once Wi-Fi and the bot work, often long after
        // boot; the delay says nothing about when anything happened.
        for state in [Present, Absent] {
            let text = event_text(&Messages::default(), &event(state, None), 10 * SEC + 30 * MIN, Readings::NONE);
            assert!(!text.contains("тому"), "{text}");
        }
    }

    fn battery_messages() -> Messages {
        let body = form_with(&[
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
        let text = event_text(&battery_messages(), &event(Absent, Some(MIN)), 10 * SEC, Readings { bms: Some(&reading()), ..Readings::NONE });
        assert_eq!(text, "Дача: батарея 84%, 13.28 В");
    }

    #[test]
    fn battery_voltage_comes_from_the_divider() {
        let body = form_with(&[("device_name", "Дача"), ("lost", "{device}: акумулятор {battery_voltage} В")]);
        let messages = Messages::from_form(&body).unwrap();
        let lost = event(Absent, Some(MIN));
        let measured = Readings { voltage_mv: Some(52_020), ..Readings::NONE };
        assert_eq!(event_text(&messages, &lost, 10 * SEC, measured), "Дача: акумулятор 52.02 В");
        assert_eq!(event_text(&messages, &lost, 10 * SEC, Readings::NONE), "Дача: акумулятор 🤷‍♂️ В");
    }

    #[test]
    fn missing_battery_reading_is_a_shrug() {
        let text = event_text(&battery_messages(), &event(Absent, Some(MIN)), 10 * SEC, Readings::NONE);
        assert_eq!(text, "Дача: батарея 🤷‍♂️%, 🤷‍♂️ В");
    }

    #[test]
    fn the_battery_is_read_only_for_texts_that_show_it() {
        let lost = Notice::Power(event(Absent, Some(MIN)));
        let returned = Notice::Power(event(Present, Some(MIN)));
        assert!(needs_bms(&battery_messages(), &lost));
        assert!(!needs_bms(&battery_messages(), &returned));
        assert!(!needs_bms(&battery_messages(), &Notice::Address("10.0.0.2".into())));
        assert!(needs_bms(&Messages::default(), &lost));
    }

    #[test]
    fn default_texts_show_the_bms_or_else_the_divider() {
        let lost = event(Absent, Some(5 * MIN));
        let both = Readings { bms: Some(&reading()), voltage_mv: Some(52_020) };
        let divider = Readings { voltage_mv: Some(52_020), ..Readings::NONE };
        let text = |readings| event_text(&Messages::default(), &lost, 10 * SEC, readings);
        assert_eq!(text(both), "🔌 Дім: світло зникло.\nБуло 5 хв.\n🔋 84% · 13.28 В · 1.12 кВт·год");
        assert_eq!(text(divider), "🔌 Дім: світло зникло.\nБуло 5 хв.\n🔋 52.02 В");
        assert_eq!(text(Readings::NONE), "🔌 Дім: світло зникло.\nБуло 5 хв.");
    }

    #[test]
    fn every_default_text_ends_with_the_battery() {
        let divider = Readings { voltage_mv: Some(52_020), ..Readings::NONE };
        for (state, lasted) in [(Present, None), (Absent, None), (Present, Some(MIN)), (Absent, Some(MIN))] {
            let text = event_text(&Messages::default(), &event(state, lasted), 10 * SEC, divider);
            assert!(text.ends_with("\n🔋 52.02 В"), "{text}");
        }
    }

    fn preview_form(text: &str, template: &str) -> String {
        form::encode(&[("device_name", "Дача"), ("text", text), ("template", template)])
    }

    #[test]
    fn preview_fills_a_sample_length_and_the_measured_values() {
        let body = preview_form("lost", "{device}{duration?: {duration}}, {battery_voltage} В");
        let measured = Readings { voltage_mv: Some(52_020), ..Readings::NONE };
        assert_eq!(preview(&body, measured), Ok("Дача: 2 год 15 хв, 52.02 В".to_string()));
    }

    #[test]
    fn preview_shows_what_is_missing_as_the_message_would() {
        let body = preview_form("start_absent", "{device}{bms_soc?, заряд {bms_soc}%}, {battery_voltage} В");
        assert_eq!(preview(&body, Readings::NONE), Ok("Дача, 🤷‍♂️ В".to_string()));
    }

    #[test]
    fn preview_explains_a_bad_template() {
        let error = preview(&preview_form("start_present", "{duration}"), Readings::NONE).unwrap_err();
        assert!(error.contains("{duration}"), "{error}");
        assert!(preview(&preview_form("nope", "x"), Readings::NONE).is_err());
    }

    #[test]
    fn delivery_within_a_minute_is_not_late() {
        let text = event_text(&Messages::default(), &event(Present, None), 10 * SEC + LATE_AFTER_MS - 1, Readings::NONE);
        assert!(!text.contains("тому"), "{text}");
    }
}
