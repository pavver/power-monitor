//! User-editable message templates with `{device}`, `{duration}` and battery
//! (`{bms_soc}`, … see `jk::VARIABLES`) placeholders.

use std::fmt;

use crate::jk;

const MAX_CHARS: usize = 500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateError {
    Empty,
    TooLong,
    /// `{duration}` used where no previous state exists (start-up messages).
    NoDuration,
    Unknown,
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            TemplateError::Empty => "текст не може бути порожнім",
            TemplateError::TooLong => "текст довший за 500 символів",
            TemplateError::NoDuration => "{duration} тут недоступний — при запуску тривалості ще немає",
            TemplateError::Unknown => "невідома змінна — дозволені {device}, {duration} і {bms_…} зі списку",
        })
    }
}

/// Checks a template; `with_duration` says whether `{duration}` is available.
pub fn validate(template: &str, with_duration: bool) -> Result<(), TemplateError> {
    if template.trim().is_empty() {
        return Err(TemplateError::Empty);
    }
    if template.chars().count() > MAX_CHARS {
        return Err(TemplateError::TooLong);
    }
    placeholders(template).try_for_each(|name| match name {
        "device" => Ok(()),
        "duration" if with_duration => Ok(()),
        "duration" => Err(TemplateError::NoDuration),
        name if jk::VARIABLES.contains(&name) => Ok(()),
        _ => Err(TemplateError::Unknown),
    })
}

/// Fills the placeholders named in `values`; anything else stays as written.
pub fn render(template: &str, values: &[(&str, &str)]) -> String {
    // One pass, so an inserted value is never read as a placeholder.
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let filled = after.split_once('}').and_then(|(name, tail)| {
            values.iter().find(|(key, _)| *key == name).map(|(_, value)| (*value, tail))
        });
        match filled {
            Some((value, tail)) => {
                out.push_str(value);
                rest = tail;
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether `template` shows any battery value.
pub fn uses_bms(template: &str) -> bool {
    placeholders(template).any(|name| jk::VARIABLES.contains(&name))
}

/// Names written as `{name}`: an ASCII letter, then letters, digits and underscores.
fn placeholders(template: &str) -> impl Iterator<Item = &str> {
    template.split('{').skip(1).filter_map(|rest| {
        let (name, _) = rest.split_once('}')?;
        let is_name = name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic())
            && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        is_name.then_some(name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALUES: [(&str, &str); 3] = [("device", "Дім"), ("duration", "5 хв"), ("bms_soc", "84")];

    #[test]
    fn fills_placeholders() {
        assert_eq!(render("{device}: {duration}, {device}", &VALUES), "Дім: 5 хв, Дім");
        assert_eq!(render("🔋 {bms_soc}%", &VALUES), "🔋 84%");
    }

    #[test]
    fn inserted_values_are_not_expanded_again() {
        assert_eq!(render("{device}", &[("device", "{duration}"), ("duration", "5 хв")]), "{duration}");
    }

    #[test]
    fn leaves_plain_text_lone_braces_and_unknown_names_alone() {
        assert_eq!(render("a { b } { {x} {", &VALUES), "a { b } { {x} {");
    }

    #[test]
    fn accepts_battery_values_in_every_message() {
        assert_eq!(validate("{bms_soc}% {bms_voltage} В {bms_cell_delta}", false), Ok(()));
        assert_eq!(validate("{bms_nonsense}", false), Err(TemplateError::Unknown));
        assert_eq!(validate("{bms_temp3}", false), Err(TemplateError::Unknown));
    }

    #[test]
    fn knows_when_the_battery_is_needed() {
        assert!(uses_bms("Заряд {bms_soc}%"));
        assert!(!uses_bms("{device}: {duration}"));
        assert!(!uses_bms("bms_soc без дужок"));
        assert!(uses_bms("Датчик {bms_temp1}°"));
    }

    #[test]
    fn accepts_known_placeholders() {
        assert_eq!(validate("🟢 {device}: світло є", false), Ok(()));
        assert_eq!(validate("Не було {duration} на {device}", true), Ok(()));
        assert_eq!(validate("{ просто дужки }", false), Ok(()));
    }

    #[test]
    fn rejects_empty_and_long_text() {
        assert_eq!(validate("  \n", true), Err(TemplateError::Empty));
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
