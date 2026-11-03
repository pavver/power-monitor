//! User-editable message templates with `{device}`, `{duration}`,
//! `{battery_voltage}` and BMS (`{bms_soc}`, … see `jk::VARIABLES`) placeholders.
//!
//! `{name?text}` is a section: `text` (which may hold placeholders and
//! sections) shows only when `name` has a value, so a missing value can
//! take its whole sentence away instead of showing a shrug; `{!name?text}`
//! shows `text` only when `name` has none, so a second value can stand in
//! for the first. `\n` in the
//! text is a line break, for fields where a real one is awkward to type.

use std::fmt;

use crate::jk;

const MAX_CHARS: usize = 500;

/// The battery voltage measured through the divider.
pub const BATTERY_VOLTAGE: &str = "battery_voltage";

/// Shown for a placeholder whose value is missing, e.g. a length nobody
/// measured or a battery value the BMS did not give.
pub const UNKNOWN: &str = "🤷‍♂️";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateError {
    TooLong,
    /// `{duration}` used where no previous state exists (start-up messages).
    NoDuration,
    Unknown,
    /// A `{name?` section without its closing `}`.
    Unclosed,
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TemplateError::TooLong => "текст довший за 500 символів",
            TemplateError::NoDuration => "{duration} тут недоступний — при запуску тривалості ще немає",
            TemplateError::Unknown => "невідома змінна — дозволені {device}, {duration}, {battery_voltage} і {bms_…} зі списку",
            TemplateError::Unclosed => "умовний фрагмент {змінна?текст} не закрито дужкою }",
        })
    }
}

/// Checks a template; `with_duration` says whether `{duration}` is available.
pub fn validate(template: &str, with_duration: bool) -> Result<(), TemplateError> {
    if template.chars().count() > MAX_CHARS {
        return Err(TemplateError::TooLong);
    }
    names(&parse(template)).into_iter().try_for_each(|name| match name {
        None => Err(TemplateError::Unclosed),
        Some("device") => Ok(()),
        Some("duration") if with_duration => Ok(()),
        Some("duration") => Err(TemplateError::NoDuration),
        Some(BATTERY_VOLTAGE) => Ok(()),
        Some(name) if jk::VARIABLES.contains(&name) => Ok(()),
        Some(_) => Err(TemplateError::Unknown),
    })
}

/// Fills the placeholders named in `values`, a missing (`None`) value as a
/// shrug, and keeps the sections whose value is there; unnamed placeholders
/// stay as written.
pub fn render(template: &str, values: &[(&str, Option<&str>)]) -> String {
    let mut out = String::with_capacity(template.len());
    // Values are inserted after parsing, so one is never read as a placeholder.
    write(&mut out, &parse(template), values);
    out
}

/// Whether `template` shows any battery value.
pub fn uses_bms(template: &str) -> bool {
    names(&parse(template)).into_iter().flatten().any(|name| jk::VARIABLES.contains(&name))
}

enum Part<'a> {
    Text(&'a str),
    Value(&'a str),
    Section(&'a str, Shown, Vec<Part<'a>>),
    /// A `{name?` never closed; shown as written.
    Unclosed,
}

/// When a section's text shows: `{name?` or `{!name?`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shown {
    Known,
    Missing,
}

fn parse(template: &str) -> Vec<Part<'_>> {
    parse_until(template, false).0
}

/// Parts up to the `}` that closes a section when `inside` one, and the text
/// after that `}` (`None` if it never came).
fn parse_until(mut rest: &str, inside: bool) -> (Vec<Part<'_>>, Option<&str>) {
    let mut parts = Vec::new();
    while let Some(at) = rest.find(|c| c == '{' || (inside && c == '}')) {
        parts.push(Part::Text(&rest[..at]));
        let after = &rest[at + 1..];
        if rest[at..].starts_with('}') {
            return (parts, Some(after));
        }
        if let Some((name, shown, body)) = section_start(after) {
            if let (body, Some(tail)) = parse_until(body, true) {
                parts.push(Part::Section(name, shown, body));
                rest = tail;
                continue;
            }
            parts.push(Part::Unclosed);
            rest = after;
            continue;
        }
        if let Some((name, tail)) = name_at(after).and_then(|(name, tail)| Some((name, tail.strip_prefix('}')?))) {
            parts.push(Part::Value(name));
            rest = tail;
            continue;
        }
        parts.push(Part::Text("{"));
        rest = after;
    }
    parts.push(Part::Text(rest));
    (parts, None)
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

fn write(out: &mut String, parts: &[Part], values: &[(&str, Option<&str>)]) {
    let value = |name: &str| values.iter().find(|(key, _)| *key == name).map(|(_, value)| *value);
    for part in parts {
        match part {
            Part::Text(text) => *out += &text.replace("\\n", "\n"),
            Part::Value(name) => match value(name) {
                Some(value) => out.push_str(value.unwrap_or(UNKNOWN)),
                None => *out += &format!("{{{name}}}"),
            },
            Part::Section(name, shown, body) => {
                let known = matches!(value(name), Some(Some(_)));
                if known == (*shown == Shown::Known) {
                    write(out, body, values);
                }
            }
            Part::Unclosed => out.push('{'),
        }
    }
}

/// Every name used, in values and sections; `None` for an unclosed section.
fn names<'a>(parts: &[Part<'a>]) -> Vec<Option<&'a str>> {
    parts
        .iter()
        .flat_map(|part| match part {
            Part::Text(_) => vec![],
            Part::Value(name) => vec![Some(*name)],
            Part::Section(name, _, body) => [vec![Some(*name)], names(body)].concat(),
            Part::Unclosed => vec![None],
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALUES: [(&str, Option<&str>); 3] = [("device", Some("Дім")), ("duration", Some("5 хв")), ("bms_soc", Some("84"))];

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
        assert_eq!(validate("{!device}", true), Ok(()));
    }

    #[test]
    fn checks_names_of_negated_sections() {
        assert_eq!(validate("{!bms_soc?{battery_voltage} В}", false), Ok(()));
        assert_eq!(validate("{!duration?a}", false), Err(TemplateError::NoDuration));
        assert_eq!(validate("{!time?a}", true), Err(TemplateError::Unknown));
        assert_eq!(validate("{!device?a", true), Err(TemplateError::Unclosed));
        assert!(uses_bms("{!bms_soc?нема BMS}"));
    }

    #[test]
    fn checks_names_inside_and_of_sections() {
        assert_eq!(validate("{duration?Не було {duration}.}", true), Ok(()));
        assert_eq!(validate("{bms_soc?{bms_soc}%}", false), Ok(()));
        assert_eq!(validate("{duration?a}", false), Err(TemplateError::NoDuration));
        assert_eq!(validate("{device?{time}}", true), Err(TemplateError::Unknown));
        assert_eq!(validate("{time?a}", true), Err(TemplateError::Unknown));
    }

    #[test]
    fn rejects_an_unclosed_section() {
        assert_eq!(validate("{duration?Не було {duration}.", true), Err(TemplateError::Unclosed));
    }

    #[test]
    fn a_battery_value_in_a_section_needs_the_battery() {
        assert!(uses_bms("{bms_soc?Заряд {bms_soc}%}"));
        assert!(uses_bms("{device?{bms_soc}}"));
    }

    #[test]
    fn accepts_battery_values_in_every_message() {
        assert_eq!(validate("{bms_soc}% {bms_voltage} В {bms_energy}", false), Ok(()));
        assert_eq!(validate("{battery_voltage} В", false), Ok(()));
        assert_eq!(validate("{bms_nonsense}", false), Err(TemplateError::Unknown));
        assert_eq!(validate("{bms_temp1}", false), Err(TemplateError::Unknown));
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
        assert_eq!(validate("⚡ {device}: світло є", false), Ok(()));
        assert_eq!(validate("Не було {duration} на {device}", true), Ok(()));
        assert_eq!(validate("{ просто дужки }", false), Ok(()));
    }

    #[test]
    fn rejects_long_text() {
        assert_eq!(validate(&"я".repeat(501), true), Err(TemplateError::TooLong));
        assert_eq!(validate(&"я".repeat(500), true), Ok(()));
    }

    #[test]
    fn rejects_duration_where_it_does_not_exist() {
        assert_eq!(validate("{duration}", false), Err(TemplateError::NoDuration));
    }

    #[test]
    fn rejects_unknown_placeholders() {
        assert_eq!(validate("{time}", true), Err(TemplateError::Unknown));
        assert_eq!(validate("{Device}", true), Err(TemplateError::Unknown));
    }
}
