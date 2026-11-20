//! Generic text template engine.
//!
//! Evaluates `{name}` placeholders, `{<condition>?text}` sections, and `{@name}` template calls.
//!
//! # Syntax
//! - `{name}`: placeholder filled with the corresponding value (or `UNKNOWN` if missing).
//! - `{@name}`: renders the template named `name` from a [`Library`] in place.
//!   Templates can call other templates up to [`MAX_DEPTH`] (4) levels deep.
//!   Missing templates or calls beyond [`MAX_DEPTH`] write [`UNKNOWN`].
//!   Calls inside hidden conditional sections do not read from the library.
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
pub use crate::library::{calls, check_calls, template_name, CallError, Library, NoTemplates};

/// Maximum characters allowed in a template.
pub const MAX_CHARS: usize = 256;
/// Maximum bytes allowed in a template.
pub const MAX_BYTES: usize = 512;
/// Maximum nesting depth for template calls (the message's own text is depth 0).
pub const MAX_DEPTH: usize = 4;

/// Shown for a placeholder whose value is missing, or a failed template call.
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
    /// A template with this name does not exist.
    NoTemplate,
    /// Template name is not 1..=15 characters of [a-z0-9_].
    BadTemplateName,
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
            TemplateError::NoTemplate => f.write_str("шаблону з такою назвою немає"),
            TemplateError::BadTemplateName => {
                f.write_str("назва шаблону — латинські малі літери, цифри та _, до 15 символів")
            }
        }
    }
}

/// Checks a template against limits, verifies placeholder/section names with `check_name`,
/// and verifies template calls `{@name}` with `has_template`.
pub fn validate(
    template: &str,
    check_name: &dyn Fn(&str) -> Option<TemplateError>,
    has_template: &dyn Fn(&str) -> bool,
) -> Result<(), TemplateError> {
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
            Event::Call(name) => {
                if fault.get().is_none() && !has_template(name) {
                    fault.set(Some(TemplateError::NoTemplate));
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

/// Renders `template` with `values` and template expansion through `library`.
///
/// Writes directly to `out`. Missing templates or calls deeper than `MAX_DEPTH`
/// output `UNKNOWN`.
pub fn render_with(
    template: &str,
    values: &[(&str, Option<&str>)],
    library: &dyn Library,
    out: &mut dyn fmt::Write,
) -> fmt::Result {
    render_depth(template, values, library, 0, out)
}

fn render_depth(
    template: &str,
    values: &[(&str, Option<&str>)],
    library: &dyn Library,
    depth: usize,
    out: &mut dyn fmt::Write,
) -> fmt::Result {
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
                Event::Call(name) => {
                    if depth >= MAX_DEPTH {
                        out.write_str(UNKNOWN)
                    } else {
                        let mut read_called = false;
                        let found = library.read(name, &mut |callee_text| {
                            read_called = true;
                            result = render_depth(callee_text, values, library, depth + 1, out);
                        });
                        if result.is_err() {
                            return;
                        }
                        if !found || !read_called {
                            out.write_str(UNKNOWN)
                        } else {
                            Ok(())
                        }
                    }
                }
                Event::Unclosed => out.write_char('{'),
                Event::Error(TemplateError::BadTemplateName) => out.write_str(UNKNOWN),
                Event::Error(_) => Ok(()),
            };
        },
        &mut |condition| condition.holds(value(condition.name()).flatten()),
    );
    result
}

/// Fills the placeholders named in `values`, a missing (`None`) value as a
/// shrug, and keeps the sections whose value is there; unnamed placeholders
/// stay as written. Writes directly to `out`.
pub fn render_to(template: &str, values: &[(&str, Option<&str>)], out: &mut dyn fmt::Write) -> fmt::Result {
    render_with(template, values, &NoTemplates, out)
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

/// Whether any placeholder or section in `template`, or in any template it calls via `library`,
/// satisfies `wanted`.
pub fn uses_with(template: &str, library: &dyn Library, wanted: &dyn Fn(&str) -> bool) -> bool {
    fn uses_depth(
        template: &str,
        library: &dyn Library,
        wanted: &dyn Fn(&str) -> bool,
        visited: &mut Vec<String>,
        depth: usize,
    ) -> bool {
        if depth > MAX_DEPTH {
            return false;
        }
        let matched = Cell::new(false);
        walk(
            template,
            &mut |event| match event {
                Event::Value(name) => {
                    if wanted(name) {
                        matched.set(true);
                    }
                }
                Event::Call(name) => {
                    if matched.get() {
                        return;
                    }
                    if !visited.iter().any(|v| v == name) {
                        visited.push(name.to_string());
                        library.read(name, &mut |text| {
                            if uses_depth(text, library, wanted, visited, depth + 1) {
                                matched.set(true);
                            }
                        });
                        visited.pop();
                    }
                }
                _ => {}
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

    let mut visited = Vec::new();
    uses_depth(template, library, wanted, &mut visited, 0)
}

/// Whether any placeholder or section name in `template` satisfies `wanted`.
pub fn uses(template: &str, wanted: &dyn Fn(&str) -> bool) -> bool {
    uses_with(template, &NoTemplates, wanted)
}

/// Runs `on` for every `{@name}` in `template`, entering every section.
pub(crate) fn each_call<'a>(template: &'a str, on: &mut dyn FnMut(&'a str)) {
    walk(template, &mut |event| if let Event::Call(name) = event { on(name) }, &mut |_| true);
}

enum Event<'a> {
    Text(&'a str),
    Value(&'a str),
    Call(&'a str),
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
        if let Some(call_after) = after.strip_prefix('@') {
            if let Some(close) = call_after.find('}') {
                let name = &call_after[..close];
                if !name.contains('{') {
                    let tail = &call_after[close + 1..];
                    if template_name(name) {
                        if active {
                            on(Event::Call(name));
                        }
                    } else if active {
                        on(Event::Error(TemplateError::BadTemplateName));
                    }
                    rest = tail;
                    continue;
                }
            }
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
    let delim = after.find(['{', '}', '?'])?;
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
            let _ = validate(&template, &|_| None, &|_| false);
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
        assert_eq!(validate("{!name}", &|_| None, &|_| false), Ok(()));
    }

    #[test]
    fn rejects_an_unclosed_section() {
        assert_eq!(validate("{duration?Не було {duration}.", &|_| None, &|_| false), Err(TemplateError::Unclosed));
    }

    #[test]
    fn rejects_long_text() {
        assert_eq!(validate(&"я".repeat(257), &|_| None, &|_| false), Err(TemplateError::TooLong));
        assert_eq!(validate(&"я".repeat(256), &|_| None, &|_| false), Ok(()));
        // 4 bytes each: 128 fit in 512 bytes, 129 do not.
        assert_eq!(validate(&"🔋".repeat(129), &|_| None, &|_| false), Err(TemplateError::TooBig));
        assert_eq!(validate(&"🔋".repeat(128), &|_| None, &|_| false), Ok(()));
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
        assert_eq!(validate("{x<abc?…}", &|_| None, &|_| false), Err(TemplateError::NotANumber));
        assert_eq!(validate("{x<?…}", &|_| None, &|_| false), Err(TemplateError::NotANumber));
        assert_eq!(validate("{x<=xyz?…}", &|_| None, &|_| false), Err(TemplateError::NotANumber));
        assert_eq!(validate("{x>bad?…}", &|_| None, &|_| false), Err(TemplateError::NotANumber));
        assert_eq!(validate("{x>=bad?…}", &|_| None, &|_| false), Err(TemplateError::NotANumber));
        assert_eq!(TemplateError::NotANumber.to_string(), "у порівнянні після <, >, <=, >= має бути число");
    }

    #[test]
    fn unknown_name_in_condition() {
        let check = |name: &str| if name == "known" { None } else { Some(TemplateError::Unknown) };
        assert_eq!(validate("{known<20?…}", &check, &|_| false), Ok(()));
        assert_eq!(validate("{unknown<20?…}", &check, &|_| false), Err(TemplateError::Unknown));
        assert_eq!(validate("{!unknown=on?…}", &check, &|_| false), Err(TemplateError::Unknown));
        assert_eq!(validate("{unknown?…}", &check, &|_| false), Err(TemplateError::Unknown));
    }

    #[test]
    fn template_names_are_1_to_15_bytes_of_lowercase_alphanumeric_or_underscore() {
        assert!(template_name("battery"));
        assert!(template_name("b"));
        assert!(template_name("b_123"));
        assert!(template_name("123456789012345"));
        assert!(!template_name(""));
        assert!(!template_name("1234567890123456")); // 16 bytes
        assert!(!template_name("Bad-Name"));
        assert!(!template_name("battery "));
        assert!(!template_name(" батарея"));
        assert!(!template_name("UPPER"));
    }

    #[test]
    fn call_renders_callee() {
        let lib = [("battery", "48 V")];
        let mut out = String::new();
        render_with("⚡ {@battery}", &[], &lib, &mut out).unwrap();
        assert_eq!(out, "⚡ 48 V");
    }

    #[test]
    fn nested_calls_render() {
        let lib = [
            ("header", "{@title}: {@battery}"),
            ("title", "Живлення"),
            ("battery", "52 В"),
        ];
        let mut out = String::new();
        render_with("{@header}", &[], &lib, &mut out).unwrap();
        assert_eq!(out, "Живлення: 52 В");
    }

    #[test]
    fn values_visible_inside_callee() {
        let lib = [("battery", "{bms_soc}% ({bms_voltage} В)")];
        let values = [("bms_soc", Some("80")), ("bms_voltage", Some("51.2"))];
        let mut out = String::new();
        render_with("{@battery}", &values, &lib, &mut out).unwrap();
        assert_eq!(out, "80% (51.2 В)");
    }

    #[test]
    fn missing_callee_renders_unknown() {
        let mut out = String::new();
        render_with("{@missing}", &[], &NoTemplates, &mut out).unwrap();
        assert_eq!(out, UNKNOWN);
    }

    #[test]
    fn depth_max_depth_renders_and_max_depth_plus_one_shows_unknown() {
        // Message is depth 0.
        // d1 is depth 1, d2 is depth 2, d3 is depth 3, d4 is depth 4 (MAX_DEPTH).
        let lib_ok = [
            ("d1", "{@d2}"),
            ("d2", "{@d3}"),
            ("d3", "{@d4}"),
            ("d4", "OK"),
        ];
        let mut out = String::new();
        render_with("{@d1}", &[], &lib_ok, &mut out).unwrap();
        assert_eq!(out, "OK");

        // d5 would be depth 5 (MAX_DEPTH + 1) -> writes UNKNOWN.
        let lib_deep = [
            ("d1", "{@d2}"),
            ("d2", "{@d3}"),
            ("d3", "{@d4}"),
            ("d4", "{@d5}"),
            ("d5", "TOO DEEP"),
        ];
        let mut out_deep = String::new();
        render_with("{@d1}", &[], &lib_deep, &mut out_deep).unwrap();
        assert_eq!(out_deep, UNKNOWN);
    }

    struct CountingLibrary<'a> {
        inner: &'a [(&'a str, &'a str)],
        reads: Cell<usize>,
    }

    impl<'a> Library for CountingLibrary<'a> {
        fn read(&self, name: &str, with: &mut dyn FnMut(&str)) -> bool {
            self.reads.set(self.reads.get() + 1);
            self.inner.read(name, with)
        }
    }

    #[test]
    fn hidden_section_does_not_read_the_library() {
        let lib_data = [("battery", "48 V")];
        let counting_lib = CountingLibrary {
            inner: &lib_data,
            reads: Cell::new(0),
        };
        let mut out = String::new();
        let values = [("bms_soc", None)];
        render_with("{bms_soc?{@battery}}", &values, &counting_lib, &mut out).unwrap();
        assert_eq!(out, "");
        assert_eq!(counting_lib.reads.get(), 0);
    }

    #[test]
    fn validate_no_template_and_bad_template_name() {
        let has = |n: &str| n == "known_tpl";
        assert_eq!(validate("{@known_tpl}", &|_| None, &has), Ok(()));
        assert_eq!(validate("{@missing}", &|_| None, &has), Err(TemplateError::NoTemplate));
        assert_eq!(TemplateError::NoTemplate.to_string(), "шаблону з такою назвою немає");

        assert_eq!(validate("{@Bad-Name}", &|_| None, &has), Err(TemplateError::BadTemplateName));
        assert_eq!(validate("{@toolongname12345678}", &|_| None, &has), Err(TemplateError::BadTemplateName));
        assert_eq!(validate("{@}", &|_| None, &has), Err(TemplateError::BadTemplateName));
        assert_eq!(
            TemplateError::BadTemplateName.to_string(),
            "назва шаблону — латинські малі літери, цифри та _, до 15 символів"
        );
    }

    #[test]
    fn check_calls_validates_cycles_and_depth() {
        // Direct self-call
        let err = check_calls("a", "{@a}", &NoTemplates);
        assert_eq!(err, Err(CallError::Cycle(vec!["a".into(), "a".into()])));
        assert_eq!(err.unwrap_err().to_string(), "шаблони викликають один одного по колу: a → a");

        // a -> b -> a cycle
        let lib_cycle = [("b", "{@a}")];
        let err_cycle = check_calls("a", "{@b}", &lib_cycle);
        assert_eq!(err_cycle, Err(CallError::Cycle(vec!["a".into(), "b".into(), "a".into()])));
        assert_eq!(err_cycle.unwrap_err().to_string(), "шаблони викликають один одного по колу: a → b → a");

        // Chain too deep: a -> b -> c -> d -> e (5 templates > MAX_DEPTH of 4)
        let lib_deep = [
            ("b", "{@c}"),
            ("c", "{@d}"),
            ("d", "{@e}"),
            ("e", "leaf"),
        ];
        let err_deep = check_calls("a", "{@b}", &lib_deep);
        assert_eq!(
            err_deep,
            Err(CallError::TooDeep(vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into()]))
        );
        assert_eq!(
            err_deep.unwrap_err().to_string(),
            "забагато вкладених викликів: a → b → c → d → e (можна 4)"
        );

        // Diamond: a calls b and c, both call d (OK!)
        let lib_diamond = [
            ("b", "{@d}"),
            ("c", "{@d}"),
            ("d", "leaf"),
        ];
        assert_eq!(check_calls("a", "{@b} {@c}", &lib_diamond), Ok(()));

        // Replacing text of an existing template that would close a cycle is caught
        let lib_existing = [
            ("a", "leaf"),
            ("b", "{@a}"),
        ];
        // Replacing "a" with "{@b}" closes the cycle: a -> b -> a
        let err_replace = check_calls("a", "{@b}", &lib_existing);
        assert_eq!(err_replace, Err(CallError::Cycle(vec!["a".into(), "b".into(), "a".into()])));

        // Message text (depth 0, name = "")
        assert_eq!(check_calls("", "{@b} {@c}", &lib_diamond), Ok(()));
        let lib_msg_deep = [
            ("b", "{@c}"),
            ("c", "{@d}"),
            ("d", "{@e}"),
            ("e", "{@f}"),
            ("f", "leaf"),
        ];
        let err_msg_deep = check_calls("", "{@b}", &lib_msg_deep);
        assert_eq!(
            err_msg_deep,
            Err(CallError::TooDeep(vec!["b".into(), "c".into(), "d".into(), "e".into(), "f".into()]))
        );
    }

    #[test]
    fn calls_iterator_finds_all_calls_including_sections() {
        let tpl = "Start {@battery} mid {bms_soc?{@details}} end {!mains?{@warning}} {@bad-name} {@}";
        let found: Vec<&str> = calls(tpl);
        assert_eq!(found, vec!["battery", "details", "warning"]);
    }

    #[test]
    fn uses_with_follows_calls() {
        let lib = [
            ("battery", "{bms_soc}% ({bms_voltage} В)"),
            ("simple", "Привіт"),
        ];
        assert!(uses_with("{@battery}", &lib, &|n| n == "bms_soc"));
        assert!(uses_with("{@battery}", &lib, &|n| n == "bms_voltage"));
        assert!(!uses_with("{@battery}", &lib, &|n| n == "device"));
        assert!(!uses_with("{@simple}", &lib, &|n| n == "bms_soc"));
        assert!(!uses_with("{@battery}", &NoTemplates, &|n| n == "bms_soc"));

        // Nested uses
        let nested_lib = [
            ("outer", "{@inner}"),
            ("inner", "{secret}"),
        ];
        assert!(uses_with("{@outer}", &nested_lib, &|n| n == "secret"));
        assert!(!uses_with("{@outer}", &nested_lib, &|n| n == "other"));
    }
}
