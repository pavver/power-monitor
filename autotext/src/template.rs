//! Generic text template engine.
//!
//! Evaluates `{name}` placeholders and `{<condition>?text}` sections.
//!
//! # Conditions
//! - `{name?text}`: shows `text` when `name` has a value.
//! - `{!name?text}`: shows `text` when `name` has no value (missing).
//! - `{name=val?text}` / `{name!=val?text}`: equality test. If both sides
//!   parse as numbers (`f64`), compares numerically (e.g. `52.0` equals `52`);
//!   otherwise compares strings exactly.
//! - `{name<num?text}`, `{name<=num?text}`, `{name>num?text}`, `{name>=num?text}`:
//!   numeric ordering (`f64` comparison, `.` decimal point). If `name`'s value
//!   does not parse as `f64`, the test is false.
//! - `!condition`: inverts any condition (e.g. `{!mains=on?text}`).
//!   A missing value evaluates to false for every test except `Known` (which tests presence),
//!   and `!` flips the result.
//!
//! `\n` in the template is turned into a line break.

use std::cell::Cell;
use std::fmt;

pub use crate::condition::{self, Condition, Test};

/// Maximum characters allowed in a template.
pub const MAX_CHARS: usize = 256;
/// Maximum bytes allowed in a template.
pub const MAX_BYTES: usize = 512;

/// Shown for a placeholder whose value is missing.
pub const UNKNOWN: &str = "🤷‍♂️";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateError {
    TooLong,
    /// Within `MAX_CHARS` but over `MAX_BYTES`, i.e. many emoji.
    TooBig,
    /// A variable that is not available in this text.
    NotHere(&'static str),
    Unknown,
    /// A `{name?` section without its closing `}`.
    Unclosed,
    /// Blank where a text must be there.
    Blank,
    /// In a condition comparison after <, >, <=, >= there must be a number.
    NotANumber,
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
            TemplateError::NotANumber => f.write_str("у порівнянні після <, >, <=, >= має бути число"),
        }
    }
}

/// Checks a template against limits and verifies placeholder/section names with `check_name`.
pub fn validate(template: &str, check_name: &dyn Fn(&str) -> Option<TemplateError>) -> Result<(), TemplateError> {
    if template.chars().count() > MAX_CHARS {
        return Err(TemplateError::TooLong);
    }
    if template.len() > MAX_BYTES {
        return Err(TemplateError::TooBig);
    }
    // The first fault in the text is the one reported.
    let fault = Cell::new(None);
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
            Event::Error(err) => {
                if fault.get().is_none() {
                    fault.set(Some(err));
                }
            }
        },
        &mut |condition| {
            if fault.get().is_none() {
                fault.set(check_name(condition.name()));
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
                Event::Error(_) => Ok(()),
            };
        },
        &mut |condition| condition.holds(value(condition.name()).flatten()),
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

/// Whether any placeholder or section name in `template` satisfies `wanted`.
pub fn uses(template: &str, wanted: &dyn Fn(&str) -> bool) -> bool {
    let matched = Cell::new(false);
    walk(
        template,
        &mut |event| {
            if let Event::Value(name) = event {
                if wanted(name) {
                    matched.set(true);
                }
            }
        },
        &mut |condition| {
            if wanted(condition.name()) {
                matched.set(true);
            }
            true
        },
    );
    matched.get()
}

enum Event<'a> {
    Text(&'a str),
    Value(&'a str),
    Unclosed,
    Error(TemplateError),
}

/// Walks `template`; `section(condition) -> bool` says whether to enter a
/// section's body (when false its body is skipped, still up to its `}`).
fn walk<'a>(
    template: &'a str,
    on: &mut dyn FnMut(Event<'a>),
    section: &mut dyn FnMut(&Condition<'a>) -> bool,
) {
    walk_until(template, false, true, on, section);
}

fn walk_until<'a>(
    mut rest: &'a str,
    inside: bool,
    active: bool,
    on: &mut dyn FnMut(Event<'a>),
    section: &mut dyn FnMut(&Condition<'a>) -> bool,
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
        if let Some((cond_res, body)) = section_start(after) {
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
                    if let Err(err) = cond_res {
                        on(Event::Error(err));
                    }
                    on(Event::Unclosed);
                }
                rest = after;
                continue;
            };
            if active {
                match cond_res {
                    Ok(condition) => {
                        if section(&condition) {
                            walk_until(body, true, true, on, section);
                        }
                    }
                    Err(err) => {
                        on(Event::Error(err));
                    }
                }
            }
            rest = tail;
            continue;
        }
        if let Some((name, tail)) = condition::name_at(after).and_then(|(name, tail)| Some((name, tail.strip_prefix('}')?))) {
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

/// The condition and body of a section that starts right after a `{`.
fn section_start(after: &str) -> Option<(Result<Condition<'_>, TemplateError>, &str)> {
    let delim = after.find(|c| c == '{' || c == '}' || c == '?')?;
    if after.as_bytes()[delim] != b'?' {
        return None;
    }
    let cond_text = &after[..delim];
    let body = &after[delim + 1..];

    let after_bang = cond_text.strip_prefix('!').unwrap_or(cond_text);
    let (_name, tail) = condition::name_at(after_bang)?;

    let is_op = tail.is_empty()
        || tail.starts_with("!=")
        || tail.starts_with("<=")
        || tail.starts_with(">=")
        || tail.starts_with('=')
        || tail.starts_with('<')
        || tail.starts_with('>');

    if !is_op {
        return None;
    }

    Some((condition::parse(cond_text), body))
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALUES: [(&str, Option<&str>); 3] = [("name", Some("Дім")), ("duration", Some("5 хв")), ("level", Some("84"))];

    #[test]
    fn render_to_writes_to_output() {
        let mut out = String::new();
        render_to("⚡ {name}: {duration}", &VALUES, &mut out).unwrap();
        assert_eq!(out, "⚡ Дім: 5 хв");
    }

    #[test]
    fn an_unclosed_section_of_a_known_value_shows_once_as_written() {
        assert_eq!(render("a {duration?b {name}", &VALUES), "a {duration?b Дім");
        assert_eq!(render("{name?{duration?x}", &VALUES), "{name?x");
    }

    #[test]
    fn the_worst_templates_take_no_time() {
        let start = std::time::Instant::now();
        for template in [
            "{name?".repeat(64),
            "{name?".repeat(32) + &"}".repeat(32),
            "{a?{".repeat(128),
            "{x<20?".repeat(64),
            "{x<20?".repeat(32) + &"}".repeat(32),
            "{!mains=on?".repeat(64),
        ] {
            let _ = validate(&template, &|_| None);
            render(&template, &VALUES);
        }
        assert!(start.elapsed() < std::time::Duration::from_millis(200), "{:?}", start.elapsed());
    }

    #[test]
    fn fills_placeholders() {
        assert_eq!(render("{name}: {duration}, {name}", &VALUES), "Дім: 5 хв, Дім");
        assert_eq!(render("🔋 {level}%", &VALUES), "🔋 84%");
    }

    #[test]
    fn inserted_values_are_not_expanded_again() {
        assert_eq!(render("{name}", &[("name", Some("{duration}")), ("duration", Some("5 хв"))]), "{duration}");
    }

    #[test]
    fn leaves_plain_text_lone_braces_and_unknown_names_alone() {
        assert_eq!(render("a { b } { {x} {", &VALUES), "a { b } { {x} {");
    }

    const UNMEASURED: [(&str, Option<&str>); 2] = [("name", Some("Дім")), ("duration", None)];

    #[test]
    fn section_shows_when_its_value_is_known() {
        assert_eq!(render("Є.{duration?\nНе було {duration}.}", &VALUES), "Є.\nНе було 5 хв.");
    }

    #[test]
    fn section_disappears_when_its_value_is_missing() {
        assert_eq!(render("Є.{duration?\nНе було {duration}.}", &UNMEASURED), "Є.");
        assert_eq!(render("{name}{duration? ({duration})}", &UNMEASURED), "Дім");
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
        assert_eq!(render("{name}", &[("name", Some("a\\nb"))]), "a\\nb");
    }

    #[test]
    fn sections_nest() {
        let text = "{name?{name}{duration?: {duration}}}";
        assert_eq!(render(text, &VALUES), "Дім: 5 хв");
        assert_eq!(render(text, &UNMEASURED), "Дім");
    }

    #[test]
    fn negated_section_shows_only_when_its_value_is_missing() {
        let text = "{level?{level}%}{!level?{duration?{duration}}}";
        assert_eq!(render(text, &VALUES), "84%");
        assert_eq!(render(text, &[("level", None), ("duration", Some("5 хв"))]), "5 хв");
        assert_eq!(render(text, &[("level", None), ("duration", None)]), "");
    }

    #[test]
    fn exclamation_without_a_section_is_plain_text() {
        assert_eq!(render("{!name} {!} {!name", &VALUES), "{!name} {!} {!name");
        assert_eq!(validate("{!name}", &|_| None), Ok(()));
    }

    #[test]
    fn rejects_an_unclosed_section() {
        assert_eq!(validate("{duration?Не було {duration}.", &|_| None), Err(TemplateError::Unclosed));
    }

    #[test]
    fn rejects_long_text() {
        assert_eq!(validate(&"я".repeat(257), &|_| None), Err(TemplateError::TooLong));
        assert_eq!(validate(&"я".repeat(256), &|_| None), Ok(()));
        // 4 bytes each: 128 fit in 512 bytes, 129 do not.
        assert_eq!(validate(&"🔋".repeat(129), &|_| None), Err(TemplateError::TooBig));
        assert_eq!(validate(&"🔋".repeat(128), &|_| None), Ok(()));
    }

    #[test]
    fn uses_finds_placeholders_and_sections() {
        assert!(uses("hello {name}", &|n| n == "name"));
        assert!(uses("{opt?hello}", &|n| n == "opt"));
        assert!(uses("{!opt?hello}", &|n| n == "opt"));
        assert!(uses("{soc<20?low}", &|n| n == "soc"));
        assert!(uses("{!mains=on?off}", &|n| n == "mains"));
        assert!(!uses("hello {name}", &|n| n == "other"));
    }

    #[test]
    fn numeric_ordering_conditions() {
        // <, <=, >, >= with integers, decimals, negatives
        let template = "{soc<20?мало}{soc<=20?не більше 20}{soc>80?багато}{soc>=80?не менше 80}";
        assert_eq!(render(template, &[("soc", Some("19"))]), "малоне більше 20");
        assert_eq!(render(template, &[("soc", Some("20"))]), "не більше 20");
        assert_eq!(render(template, &[("soc", Some("50"))]), "");
        assert_eq!(render(template, &[("soc", Some("80"))]), "не менше 80");
        assert_eq!(render(template, &[("soc", Some("81"))]), "багатоне менше 80");

        // Decimals and negative numbers
        let temp_tpl = "{temp<-5.5?дуже холодно}{temp>=-1.5?тепло}";
        assert_eq!(render(temp_tpl, &[("temp", Some("-6.0"))]), "дуже холодно");
        assert_eq!(render(temp_tpl, &[("temp", Some("-5.5"))]), "");
        assert_eq!(render(temp_tpl, &[("temp", Some("-1.5"))]), "тепло");
        assert_eq!(render(temp_tpl, &[("temp", Some("0.0"))]), "тепло");

        // Non-number value in ordering test evaluates to false
        assert_eq!(render("{soc<20?так}", &[("soc", Some("abc"))]), "");
    }

    #[test]
    fn equality_on_text_and_numbers() {
        // Text equality
        let text_tpl = "{mains=on?Світло є}{mains!=on?Світла нема}";
        assert_eq!(render(text_tpl, &[("mains", Some("on"))]), "Світло є");
        assert_eq!(render(text_tpl, &[("mains", Some("off"))]), "Світла нема");

        // Number equality: "52.0" = "52"
        let num_tpl = "{v=52?52 В}{v!=52?не 52 В}";
        assert_eq!(render(num_tpl, &[("v", Some("52.0"))]), "52 В");
        assert_eq!(render(num_tpl, &[("v", Some("52"))]), "52 В");
        assert_eq!(render(num_tpl, &[("v", Some("52.5"))]), "не 52 В");

        let num_tpl2 = "{v=52.0?точно 52}";
        assert_eq!(render(num_tpl2, &[("v", Some("52"))]), "точно 52");
    }

    #[test]
    fn missing_value_with_each_operator() {
        let missing = &[("x", None)];

        // Without '!', every test is false for missing value
        assert_eq!(render("{x=on?так}", missing), "");
        assert_eq!(render("{x!=on?так}", missing), "");
        assert_eq!(render("{x<20?так}", missing), "");
        assert_eq!(render("{x<=20?так}", missing), "");
        assert_eq!(render("{x>20?так}", missing), "");
        assert_eq!(render("{x>=20?так}", missing), "");
        assert_eq!(render("{x?так}", missing), "");

        // With '!', missing value flips to true for every test
        assert_eq!(render("{!x=on?так}", missing), "так");
        assert_eq!(render("{!x!=on?так}", missing), "так");
        assert_eq!(render("{!x<20?так}", missing), "так");
        assert_eq!(render("{!x<=20?так}", missing), "так");
        assert_eq!(render("{!x>20?так}", missing), "так");
        assert_eq!(render("{!x>=20?так}", missing), "так");
        assert_eq!(render("{!x?так}", missing), "так");
    }

    #[test]
    fn nesting_condition_sections() {
        let tpl = "{mains=on?Мережа: {soc<20?Батарея розряджена ({soc}%)}}{!mains=on?Без мережі}";
        assert_eq!(render(tpl, &[("mains", Some("on")), ("soc", Some("15"))]), "Мережа: Батарея розряджена (15%)");
        assert_eq!(render(tpl, &[("mains", Some("on")), ("soc", Some("80"))]), "Мережа: ");
        assert_eq!(render(tpl, &[("mains", Some("off")), ("soc", Some("80"))]), "Без мережі");

        let tpl2 = "{mains=on?{soc<20?А}{soc>=20?Б}}{!mains=on?В}";
        assert_eq!(render(tpl2, &[("mains", Some("on")), ("soc", Some("10"))]), "А");
        assert_eq!(render(tpl2, &[("mains", Some("on")), ("soc", Some("50"))]), "Б");
        assert_eq!(render(tpl2, &[("mains", Some("off")), ("soc", Some("50"))]), "В");
    }

    #[test]
    fn bad_numbers_report_not_a_number() {
        assert_eq!(validate("{x<abc?…}", &|_| None), Err(TemplateError::NotANumber));
        assert_eq!(validate("{x<?…}", &|_| None), Err(TemplateError::NotANumber));
        assert_eq!(validate("{x<=xyz?…}", &|_| None), Err(TemplateError::NotANumber));
        assert_eq!(validate("{x>bad?…}", &|_| None), Err(TemplateError::NotANumber));
        assert_eq!(validate("{x>=bad?…}", &|_| None), Err(TemplateError::NotANumber));
        assert_eq!(TemplateError::NotANumber.to_string(), "у порівнянні після <, >, <=, >= має бути число");
    }

    #[test]
    fn unknown_name_in_condition() {
        let check = |name: &str| if name == "known" { None } else { Some(TemplateError::Unknown) };
        assert_eq!(validate("{known<20?…}", &check), Ok(()));
        assert_eq!(validate("{unknown<20?…}", &check), Err(TemplateError::Unknown));
        assert_eq!(validate("{!unknown=on?…}", &check), Err(TemplateError::Unknown));
        assert_eq!(validate("{unknown?…}", &check), Err(TemplateError::Unknown));
    }
}
