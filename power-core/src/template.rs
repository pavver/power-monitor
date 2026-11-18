//! User-editable message templates. `{device}`, `{battery_voltage}` and the
//! BMS values (`{bms_soc}`, … see `jk::VARIABLES`) fit every text; the
//! values in `OWN` only the texts that have them.
//!
//! `{name?text}` is a section: `text` (which may hold placeholders and
//! sections) shows only when `name` has a value, so a missing value can
//! take its whole sentence away instead of showing a shrug; `{!name?text}`
//! shows `text` only when `name` has none, so a second value can stand in
//! for the first. `\n` in the
//! text is a line break, for fields where a real one is awkward to type.

use std::cell::Cell;
use std::fmt;

use crate::jk;

/// A text is sent to the device as plain UTF-8 and kept whole in memory
/// and in NVS, so it is held to both: Cyrillic takes 2 bytes, emoji 4.
pub const MAX_CHARS: usize = 256;
pub const MAX_BYTES: usize = 512;

/// The battery voltage measured through the divider.
pub const BATTERY_VOLTAGE: &str = "battery_voltage";

/// Values only some texts have: how long the previous state lasted and how
/// late the message is (power changes), the new address (its own message).
pub const OWN: [&str; 3] = ["duration", "delay", "ip"];

/// Shown for a placeholder whose value is missing, e.g. a length nobody
/// measured or a battery value the BMS did not give.
pub const UNKNOWN: &str = "🤷‍♂️";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateError {
    TooLong,
    /// Within `MAX_CHARS` but over `MAX_BYTES`, i.e. many emoji.
    TooBig,
    /// One of `OWN` in a text that does not have it, e.g. `{duration}` at start-up.
    NotHere(&'static str),
    Unknown,
    /// A `{name?` section without its closing `}`.
    Unclosed,
    /// Blank where a text must be there.
    Blank,
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TemplateError::TooLong => f.write_str("текст довший за 256 символів"),
            TemplateError::TooBig => f.write_str("текст займає понад 512 байтів (емодзі — по 4 байти)"),
            TemplateError::NotHere(name) => write!(f, "змінна {{{name}}} у цьому тексті недоступна"),
            TemplateError::Unknown => f.write_str("невідома змінна — ті, що є, перелічені під текстом"),
            TemplateError::Unclosed => f.write_str("умовний фрагмент {змінна?текст} не закрито дужкою }"),
            TemplateError::Blank => f.write_str("цей текст не можна залишити порожнім — ним перевіряється бот"),
        }
    }
}

/// Checks a template for a text that has the `own` values of `OWN`.
pub fn validate(template: &str, own: &[&str]) -> Result<(), TemplateError> {
    if template.chars().count() > MAX_CHARS {
        return Err(TemplateError::TooLong);
    }
    if template.len() > MAX_BYTES {
        return Err(TemplateError::TooBig);
    }
    // The first fault in the text is the one reported.
    let fault = Cell::new(None);
    let check_name = |name: &str| match name {
        "device" | BATTERY_VOLTAGE => None,
        name if jk::VARIABLES.contains(&name) || own.contains(&name) => None,
        name => match OWN.into_iter().find(|&other| other == name) {
            Some(other) => Some(TemplateError::NotHere(other)),
            None => Some(TemplateError::Unknown),
        },
    };
    walk(
        template,
        &mut |event| match event {
            Event::Text(_) => {}
            Event::Value(name) => {
                if fault.get().is_none() {
                    fault.set(check_name(name));
                }
            }
            Event::Unclosed => {
                if fault.get().is_none() {
                    fault.set(Some(TemplateError::Unclosed));
                }
            }
        },
        &mut |name, _shown| {
            if fault.get().is_none() {
                fault.set(check_name(name));
            }
            true
        },
    );
    fault.get().map_or(Ok(()), Err)
}

/// Fills the placeholders named in `values`, a missing (`None`) value as a
/// shrug, and keeps the sections whose value is there; unnamed placeholders
/// stay as written. Writes directly to `out`.
pub fn render_to(template: &str, values: &[(&str, Option<&str>)], out: &mut dyn fmt::Write) -> fmt::Result {
    let value = |name: &str| values.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
    let mut result = Ok(());
    walk(
        template,
        &mut |event| {
            if result.is_err() {
                return;
            }
            result = match event {
                Event::Text(text) => write_text(text, out),
                Event::Value(name) => match value(name) {
                    Some(Some(val)) => out.write_str(val),
                    Some(None) => out.write_str(UNKNOWN),
                    None => {
                        out.write_char('{')
                            .and_then(|()| out.write_str(name))
                            .and_then(|()| out.write_char('}'))
                    }
                },
                Event::Unclosed => out.write_char('{'),
            };
        },
        &mut |name, shown| {
            let known = matches!(value(name), Some(Some(_)));
            known == (shown == Shown::Known)
        },
    );
    result
}

fn write_text(mut text: &str, out: &mut dyn fmt::Write) -> fmt::Result {
    while let Some(pos) = text.find("\\n") {
        out.write_str(&text[..pos])?;
        out.write_char('\n')?;
        text = &text[pos + 2..];
    }
    out.write_str(text)
}

/// Fills the placeholders named in `values`, a missing (`None`) value as a
/// shrug, and keeps the sections whose value is there; unnamed placeholders
/// stay as written.
pub fn render(template: &str, values: &[(&str, Option<&str>)]) -> String {
    let mut out = String::with_capacity(template.len());
    let _ = render_to(template, values, &mut out);
    out
}

/// Whether `template` shows any battery value.
pub fn uses_bms(template: &str) -> bool {
    let uses = Cell::new(false);
    walk(
        template,
        &mut |event| {
            if let Event::Value(name) = event {
                if jk::VARIABLES.contains(&name) {
                    uses.set(true);
                }
            }
        },
        &mut |name, _shown| {
            if jk::VARIABLES.contains(&name) {
                uses.set(true);
            }
            true
        },
    );
    uses.get()
}

/// When a section's text shows: `{name?` or `{!name?`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    Known,
    Missing,
}

enum Event<'a> {
    Text(&'a str),
    Value(&'a str),
    Unclosed,
}

/// Walks `template`; `section(name, shown) -> bool` says whether to enter a
/// section's body (when false its body is skipped, still up to its `}`).
fn walk<'a>(
    template: &'a str,
    on: &mut dyn FnMut(Event<'a>),
    section: &mut dyn FnMut(&'a str, Shown) -> bool,
) {
    walk_until(template, false, true, on, section);
}

fn walk_until<'a>(
    mut rest: &'a str,
    inside: bool,
    active: bool,
    on: &mut dyn FnMut(Event<'a>),
    section: &mut dyn FnMut(&'a str, Shown) -> bool,
) -> Option<&'a str> {
    while let Some(at) = rest.find(|c| c == '{' || (inside && c == '}')) {
        let text = &rest[..at];
        if active && !text.is_empty() {
            on(Event::Text(text));
        }
        let after = &rest[at + 1..];
        if rest[at..].starts_with('}') {
            return Some(after);
        }
        if let Some((name, shown, body)) = section_start(after) {
            // A dry walk first: an unclosed section's body is plain text, so
            // nothing of it is shown before it is known to close.
            let Some(tail) = walk_until(body, true, false, on, section) else {
                // A section running to the end leaves every section around it
                // unclosed too: each would read the same rest. Going back up at
                // once keeps nested sections from multiplying the work.
                if inside {
                    return None;
                }
                if active {
                    on(Event::Unclosed);
                }
                rest = after;
                continue;
            };
            if active && section(name, shown) {
                walk_until(body, true, true, on, section);
            }
            rest = tail;
            continue;
        }
        if let Some((name, tail)) = name_at(after).and_then(|(name, tail)| Some((name, tail.strip_prefix('}')?))) {
            if active {
                on(Event::Value(name));
            }
            rest = tail;
            continue;
        }
        if active {
            on(Event::Text("{"));
        }
        rest = after;
    }
    if active && !rest.is_empty() {
        on(Event::Text(rest));
    }
    None
}

/// The name, when it shows and the text after `?` of a section that starts
/// right after a `{`.
fn section_start(after: &str) -> Option<(&str, Shown, &str)> {
    let (shown, after) = match after.strip_prefix('!') {
        Some(after) => (Shown::Missing, after),
        None => (Shown::Known, after),
    };
    let (name, tail) = name_at(after)?;
    Some((name, shown, tail.strip_prefix('?')?))
}

/// The name `text` starts with and the text after it.
fn name_at(text: &str) -> Option<(&str, &str)> {
    let (name, tail) = text.split_at(text.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(text.len()));
    name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic()).then_some((name, tail))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a power change offers on top of the common values.
    const CHANGE: &[&str] = &["duration", "delay"];

    #[test]
    fn each_text_offers_its_own_values() {
        assert_eq!(validate("{delay?(сталося {delay} тому)}", CHANGE), Ok(()));
        assert_eq!(validate("http://{ip}/", &["ip"]), Ok(()));
        assert_eq!(validate("{ip}", CHANGE), Err(TemplateError::NotHere("ip")));
        assert_eq!(validate("{!delay?вчасно}", &["ip"]), Err(TemplateError::NotHere("delay")));
        assert_eq!(validate("{device} {battery_voltage} {bms_soc}", &[]), Ok(()));
    }

    #[test]
    fn says_which_value_is_not_here() {
        assert_eq!(TemplateError::NotHere("ip").to_string(), "змінна {ip} у цьому тексті недоступна");
    }

    const VALUES: [(&str, Option<&str>); 3] = [("device", Some("Дім")), ("duration", Some("5 хв")), ("bms_soc", Some("84"))];

    #[test]
    fn render_to_writes_to_output() {
        let mut out = String::new();
        render_to("⚡ {device}: {duration}", &VALUES, &mut out).unwrap();
        assert_eq!(out, "⚡ Дім: 5 хв");
    }

    #[test]
    fn an_unclosed_section_of_a_known_value_shows_once_as_written() {
        assert_eq!(render("a {duration?b {device}", &VALUES), "a {duration?b Дім");
        assert_eq!(render("{device?{duration?x}", &VALUES), "{device?x");
    }

    #[test]
    fn the_worst_templates_take_no_time() {
        let start = std::time::Instant::now();
        for template in ["{device?".repeat(64), "{device?".repeat(32) + &"}".repeat(32), "{a?{".repeat(128)] {
            let _ = validate(&template, CHANGE);
            render(&template, &VALUES);
        }
        assert!(start.elapsed() < std::time::Duration::from_millis(200), "{:?}", start.elapsed());
    }

    #[test]
    fn fills_placeholders() {
        assert_eq!(render("{device}: {duration}, {device}", &VALUES), "Дім: 5 хв, Дім");
        assert_eq!(render("🔋 {bms_soc}%", &VALUES), "🔋 84%");
    }

    #[test]
    fn inserted_values_are_not_expanded_again() {
        assert_eq!(render("{device}", &[("device", Some("{duration}")), ("duration", Some("5 хв"))]), "{duration}");
    }

    #[test]
    fn leaves_plain_text_lone_braces_and_unknown_names_alone() {
        assert_eq!(render("a { b } { {x} {", &VALUES), "a { b } { {x} {");
    }

    const UNMEASURED: [(&str, Option<&str>); 2] = [("device", Some("Дім")), ("duration", None)];

    #[test]
    fn section_shows_when_its_value_is_known() {
        assert_eq!(render("Є.{duration?\nНе було {duration}.}", &VALUES), "Є.\nНе було 5 хв.");
    }

    #[test]
    fn section_disappears_when_its_value_is_missing() {
        assert_eq!(render("Є.{duration?\nНе було {duration}.}", &UNMEASURED), "Є.");
        assert_eq!(render("{device}{duration? ({duration})}", &UNMEASURED), "Дім");
    }

    #[test]
    fn missing_value_outside_a_section_is_a_shrug() {
        assert_eq!(render("Було {duration}.", &UNMEASURED), "Було 🤷‍♂️.");
    }

    #[test]
    fn backslash_n_is_a_line_break() {
        assert_eq!(render("Є.{duration?\\nНе було {duration}.}", &VALUES), "Є.\nНе було 5 хв.");
        assert_eq!(render("a\\nb", &VALUES), "a\nb");
    }

    #[test]
    fn backslash_n_in_a_value_stays_as_it_is() {
        assert_eq!(render("{device}", &[("device", Some("a\\nb"))]), "a\\nb");
    }

    #[test]
    fn sections_nest() {
        let text = "{device?{device}{duration?: {duration}}}";
        assert_eq!(render(text, &VALUES), "Дім: 5 хв");
        assert_eq!(render(text, &UNMEASURED), "Дім");
    }

    #[test]
    fn negated_section_shows_only_when_its_value_is_missing() {
        let text = "{bms_soc?{bms_soc}%}{!bms_soc?{duration?{duration}}}";
        assert_eq!(render(text, &VALUES), "84%");
        assert_eq!(render(text, &[("bms_soc", None), ("duration", Some("5 хв"))]), "5 хв");
        assert_eq!(render(text, &[("bms_soc", None), ("duration", None)]), "");
    }

    #[test]
    fn exclamation_without_a_section_is_plain_text() {
        assert_eq!(render("{!device} {!} {!device", &VALUES), "{!device} {!} {!device");
        assert_eq!(validate("{!device}", CHANGE), Ok(()));
    }

    #[test]
    fn checks_names_of_negated_sections() {
        assert_eq!(validate("{!bms_soc?{battery_voltage} В}", &[]), Ok(()));
        assert_eq!(validate("{!duration?a}", &[]), Err(TemplateError::NotHere("duration")));
        assert_eq!(validate("{!time?a}", CHANGE), Err(TemplateError::Unknown));
        assert_eq!(validate("{!device?a", CHANGE), Err(TemplateError::Unclosed));
        assert!(uses_bms("{!bms_soc?нема BMS}"));
    }

    #[test]
    fn checks_names_inside_and_of_sections() {
        assert_eq!(validate("{duration?Не було {duration}.}", CHANGE), Ok(()));
        assert_eq!(validate("{bms_soc?{bms_soc}%}", &[]), Ok(()));
        assert_eq!(validate("{duration?a}", &[]), Err(TemplateError::NotHere("duration")));
        assert_eq!(validate("{device?{time}}", CHANGE), Err(TemplateError::Unknown));
        assert_eq!(validate("{time?a}", CHANGE), Err(TemplateError::Unknown));
    }

    #[test]
    fn rejects_an_unclosed_section() {
        assert_eq!(validate("{duration?Не було {duration}.", CHANGE), Err(TemplateError::Unclosed));
    }

    #[test]
    fn a_battery_value_in_a_section_needs_the_battery() {
        assert!(uses_bms("{bms_soc?Заряд {bms_soc}%}"));
        assert!(uses_bms("{device?{bms_soc}}"));
    }

    #[test]
    fn accepts_battery_values_in_every_message() {
        assert_eq!(validate("{bms_soc}% {bms_voltage} В {bms_energy}", &[]), Ok(()));
        assert_eq!(validate("{battery_voltage} В", &[]), Ok(()));
        assert_eq!(validate("{bms_nonsense}", &[]), Err(TemplateError::Unknown));
        assert_eq!(validate("{bms_temp1}", &[]), Err(TemplateError::Unknown));
    }

    #[test]
    fn knows_when_the_battery_is_needed() {
        assert!(uses_bms("Заряд {bms_soc}%"));
        assert!(!uses_bms("{device}: {duration}"));
        assert!(!uses_bms("bms_soc без дужок"));
        assert!(uses_bms("Енергія {bms_energy} кВт·год"));
    }

    #[test]
    fn accepts_known_placeholders() {
        assert_eq!(validate("⚡ {device}: світло є", &[]), Ok(()));
        assert_eq!(validate("Не було {duration} на {device}", CHANGE), Ok(()));
        assert_eq!(validate("{ просто дужки }", &[]), Ok(()));
    }

    #[test]
    fn rejects_long_text() {
        assert_eq!(validate(&"я".repeat(257), CHANGE), Err(TemplateError::TooLong));
        assert_eq!(validate(&"я".repeat(256), CHANGE), Ok(()));
        // 4 bytes each: 128 fit in 512 bytes, 129 do not.
        assert_eq!(validate(&"🔋".repeat(129), CHANGE), Err(TemplateError::TooBig));
        assert_eq!(validate(&"🔋".repeat(128), CHANGE), Ok(()));
    }

    #[test]
    fn rejects_duration_where_it_does_not_exist() {
        assert_eq!(validate("{duration}", &[]), Err(TemplateError::NotHere("duration")));
    }

    #[test]
    fn rejects_unknown_placeholders() {
        assert_eq!(validate("{time}", CHANGE), Err(TemplateError::Unknown));
        assert_eq!(validate("{Device}", CHANGE), Err(TemplateError::Unknown));
    }
}
