//! Value conditions in templates.
//!
//! Evaluates `{name}`, `{!name}`, `{name=val}`, `{name!=val}`, `{name<num}`,
//! `{name<=num}`, `{name>num}`, `{name>=num}`, and their negated `!name=...` forms.

use crate::template::TemplateError;

/// A condition on a template variable.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Condition<'a> {
    pub name: &'a str,
    pub test: Test<'a>,
    pub negated: bool,
}

/// The check applied to the variable's value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Test<'a> {
    Known,
    Eq(&'a str),
    Ne(&'a str),
    Lt(f64),
    Le(f64),
    Gt(f64),
    Ge(f64),
}

/// The name `text` starts with and the text after it.
pub fn name_at(text: &str) -> Option<(&str, &str)> {
    let (name, tail) = text.split_at(text.find(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).unwrap_or(text.len()));
    name.bytes().next().is_some_and(|b| b.is_ascii_alphabetic()).then_some((name, tail))
}

/// Parses a condition like "name", "!name", "name=on", "name!=on", "bms_soc<20", "x>=-1.5", "!mains=on".
pub fn parse(text: &str) -> Result<Condition<'_>, TemplateError> {
    if text.contains(['{', '}', '?']) {
        return Err(TemplateError::Unknown);
    }
    let (negated, after_bang) = match text.strip_prefix('!') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let (name, after_name) = name_at(after_bang).ok_or(TemplateError::Unknown)?;

    let test = if after_name.is_empty() {
        Test::Known
    } else if let Some(rhs) = after_name.strip_prefix("!=") {
        Test::Ne(rhs)
    } else if let Some(rhs) = after_name.strip_prefix("<=") {
        Test::Le(parse_number(rhs)?)
    } else if let Some(rhs) = after_name.strip_prefix(">=") {
        Test::Ge(parse_number(rhs)?)
    } else if let Some(rhs) = after_name.strip_prefix('=') {
        Test::Eq(rhs)
    } else if let Some(rhs) = after_name.strip_prefix('<') {
        Test::Lt(parse_number(rhs)?)
    } else if let Some(rhs) = after_name.strip_prefix('>') {
        Test::Gt(parse_number(rhs)?)
    } else {
        return Err(TemplateError::Unknown);
    };

    Ok(Condition { name, test, negated })
}

fn parse_number(s: &str) -> Result<f64, TemplateError> {
    match s.parse::<f64>() {
        Ok(n) if n.is_finite() => Ok(n),
        _ => Err(TemplateError::NotANumber),
    }
}

fn parse_finite(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|n| n.is_finite())
}

fn eq_values(val: &str, rhs: &str) -> bool {
    match (parse_finite(val), parse_finite(rhs)) {
        (Some(a), Some(b)) => a == b,
        _ => val == rhs,
    }
}

impl<'a> Condition<'a> {
    pub fn parse(text: &'a str) -> Result<Self, TemplateError> {
        parse(text)
    }

    pub fn name(&self) -> &str {
        self.name
    }

    pub fn test(&self) -> &Test<'a> {
        &self.test
    }

    pub fn negated(&self) -> bool {
        self.negated
    }

    pub fn holds(&self, value: Option<&str>) -> bool {
        let base = match (self.test, value) {
            (_, None) => false,
            (Test::Known, Some(_)) => true,
            (Test::Eq(rhs), Some(val)) => eq_values(val, rhs),
            (Test::Ne(rhs), Some(val)) => !eq_values(val, rhs),
            (Test::Lt(rhs), Some(val)) => parse_finite(val).is_some_and(|v| v < rhs),
            (Test::Le(rhs), Some(val)) => parse_finite(val).is_some_and(|v| v <= rhs),
            (Test::Gt(rhs), Some(val)) => parse_finite(val).is_some_and(|v| v > rhs),
            (Test::Ge(rhs), Some(val)) => parse_finite(val).is_some_and(|v| v >= rhs),
        };
        if self.negated {
            !base
        } else {
            base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_various_conditions() {
        assert_eq!(parse("name"), Ok(Condition { name: "name", test: Test::Known, negated: false }));
        assert_eq!(parse("!name"), Ok(Condition { name: "name", test: Test::Known, negated: true }));
        assert_eq!(parse("name=on"), Ok(Condition { name: "name", test: Test::Eq("on"), negated: false }));
        assert_eq!(parse("!name=on"), Ok(Condition { name: "name", test: Test::Eq("on"), negated: true }));
        assert_eq!(parse("name!=on"), Ok(Condition { name: "name", test: Test::Ne("on"), negated: false }));
        assert_eq!(parse("!name!=on"), Ok(Condition { name: "name", test: Test::Ne("on"), negated: true }));
        assert_eq!(parse("bms_soc<20"), Ok(Condition { name: "bms_soc", test: Test::Lt(20.0), negated: false }));
        assert_eq!(parse("bms_soc<=20"), Ok(Condition { name: "bms_soc", test: Test::Le(20.0), negated: false }));
        assert_eq!(parse("x>10"), Ok(Condition { name: "x", test: Test::Gt(10.0), negated: false }));
        assert_eq!(parse("x>=-1.5"), Ok(Condition { name: "x", test: Test::Ge(-1.5), negated: false }));
        assert_eq!(parse("name="), Ok(Condition { name: "name", test: Test::Eq(""), negated: false }));
        assert_eq!(parse("name!="), Ok(Condition { name: "name", test: Test::Ne(""), negated: false }));
    }

    #[test]
    fn rejects_invalid_conditions() {
        assert_eq!(parse("x<abc"), Err(TemplateError::NotANumber));
        assert_eq!(parse("x<"), Err(TemplateError::NotANumber));
        assert_eq!(parse("x<=foo"), Err(TemplateError::NotANumber));
        assert_eq!(parse("x>bar"), Err(TemplateError::NotANumber));
        assert_eq!(parse("x>=baz"), Err(TemplateError::NotANumber));
        assert_eq!(parse(""), Err(TemplateError::Unknown));
        assert_eq!(parse("!"), Err(TemplateError::Unknown));
        assert_eq!(parse("123"), Err(TemplateError::Unknown));
        assert_eq!(parse("x{y"), Err(TemplateError::Unknown));
        assert_eq!(parse("x}y"), Err(TemplateError::Unknown));
        assert_eq!(parse("x?y"), Err(TemplateError::Unknown));
    }

    #[test]
    fn holds_known() {
        let cond = parse("name").unwrap();
        assert!(cond.holds(Some("val")));
        assert!(!cond.holds(None));

        let neg = parse("!name").unwrap();
        assert!(!neg.holds(Some("val")));
        assert!(neg.holds(None));
    }

    #[test]
    fn holds_equality() {
        let eq = parse("state=on").unwrap();
        assert!(eq.holds(Some("on")));
        assert!(!eq.holds(Some("off")));
        assert!(!eq.holds(None));

        let neg_eq = parse("!state=on").unwrap();
        assert!(!neg_eq.holds(Some("on")));
        assert!(neg_eq.holds(Some("off")));
        assert!(neg_eq.holds(None));

        let num_eq = parse("volt=52.0").unwrap();
        assert!(num_eq.holds(Some("52")));
        assert!(num_eq.holds(Some("52.0")));
        assert!(!num_eq.holds(Some("52.1")));
    }

    #[test]
    fn holds_ordering() {
        let lt = parse("soc<20").unwrap();
        assert!(lt.holds(Some("19.9")));
        assert!(!lt.holds(Some("20")));
        assert!(!lt.holds(Some("20.1")));
        assert!(!lt.holds(Some("abc")));
        assert!(!lt.holds(None));

        let neg_lt = parse("!soc<20").unwrap();
        assert!(!neg_lt.holds(Some("19.9")));
        assert!(neg_lt.holds(Some("20")));
        assert!(neg_lt.holds(Some("abc")));
        assert!(neg_lt.holds(None));
    }
}
