//! User-editable message templates with `{device}` and `{duration}` placeholders.

use std::fmt;

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
            TemplateError::Unknown => "дозволені лише змінні {device} та {duration}",
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
        _ => Err(TemplateError::Unknown),
    })
}

/// Fills the placeholders. Unknown ones cannot occur after `validate`.
pub fn render(template: &str, device: &str, duration: &str) -> String {
    // Duration first: a device name may itself contain "{duration}".
    template.replace("{duration}", duration).replace("{device}", device)
}

/// Names written as `{name}`, where a name is ASCII letters and underscores.
fn placeholders(template: &str) -> impl Iterator<Item = &str> {
    template.split('{').skip(1).filter_map(|rest| {
        let (name, _) = rest.split_once('}')?;
        let is_name = !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphabetic() || b == b'_');
        is_name.then_some(name)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_placeholders() {
        assert_eq!(render("{device}: {duration}, {device}", "Дім", "5 хв"), "Дім: 5 хв, Дім");
    }

    #[test]
    fn inserted_values_are_not_expanded_again() {
        assert_eq!(render("{device}", "{duration}", "5 хв"), "{duration}");
    }

    #[test]
    fn leaves_plain_text_and_lone_braces_alone() {
        assert_eq!(render("a { b } {", "x", "y"), "a { b } {");
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
