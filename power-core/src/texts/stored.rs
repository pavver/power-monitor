//! Serialization and validation for templates, automations, and status policies.

use autotext::automation::{Action, Automation, Trigger};
use autotext::library::{self, Library};
use autotext::status::{Live, StatusConfig};
use autotext::template::{self, TemplateError};

use crate::form;
pub use crate::texts::error::TextsError;
use crate::texts::values;

/// Checks that `name` and `text` form a valid template suitable for saving.
pub fn check_template(name: &str, text: &str, library: &dyn Library) -> Result<(), TextsError> {
    if !library::template_name(name) {
        return Err(TextsError::BadTemplateName(name.to_string()));
    }
    template::validate(
        text,
        &|var| if values::known(var) { None } else { Some(TemplateError::Unknown) },
        &|tmpl| tmpl == name || library.read(tmpl, &mut |_| {}),
    )
    .map_err(TextsError::Template)?;

    library::check_calls(name, text, library).map_err(TextsError::Call)?;
    Ok(())
}

/// Encodes an automation into form-urlencoded format.
///
/// Conditions are represented by repeating the `if` field.
pub fn automation_to_form(a: &Automation) -> String {
    let mut pairs: Vec<(&str, String)> = Vec::new();
    match &a.trigger {
        Trigger::StartUp => pairs.push(("on", "start".to_string())),
        Trigger::Change { sensor } => {
            pairs.push(("on", "change".to_string()));
            pairs.push(("sensor", sensor.clone()));
        }
        Trigger::Becomes { sensor, value } => {
            pairs.push(("on", "becomes".to_string()));
            pairs.push(("sensor", sensor.clone()));
            pairs.push(("value", value.clone()));
        }
        Trigger::Step { sensor, step } => {
            pairs.push(("on", "step".to_string()));
            pairs.push(("sensor", sensor.clone()));
            pairs.push(("step", step.to_string()));
        }
        Trigger::Below { sensor, threshold, hysteresis } => {
            pairs.push(("on", "below".to_string()));
            pairs.push(("sensor", sensor.clone()));
            pairs.push(("threshold", threshold.to_string()));
            pairs.push(("hysteresis", hysteresis.to_string()));
        }
        Trigger::Above { sensor, threshold, hysteresis } => {
            pairs.push(("on", "above".to_string()));
            pairs.push(("sensor", sensor.clone()));
            pairs.push(("threshold", threshold.to_string()));
            pairs.push(("hysteresis", hysteresis.to_string()));
        }
        Trigger::At { minute } => {
            pairs.push(("on", "at".to_string()));
            pairs.push(("at", format!("{:02}:{:02}", minute / 60, minute % 60)));
        }
        Trigger::Every { seconds } => {
            pairs.push(("on", "every".to_string()));
            pairs.push(("every", seconds.to_string()));
        }
    }
    for cond in &a.conditions {
        pairs.push(("if", cond.clone()));
    }
    let do_str = match a.action {
        Action::Send => "send",
        Action::UpdateStatus => "status",
    };
    pairs.push(("do", do_str.to_string()));
    pairs.push(("text", a.text.clone()));

    let borrowed: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (*k, v.as_str())).collect();
    form::encode(&borrowed)
}

/// Parses and validates an automation rule from form-urlencoded body.
pub fn automation_from_form(
    body: &str,
    library: &dyn Library,
    has_template: &dyn Fn(&str) -> bool,
) -> Result<Automation, TextsError> {
    let pairs = form::parse(body);
    let on = form::field(&pairs, "on");
    if on.is_empty() {
        return Err(TextsError::MissingField("on"));
    }
    let sensor = form::field(&pairs, "sensor");

    let parse_num = |field_name: &'static str| -> Result<f64, TextsError> {
        let val = form::field(&pairs, field_name);
        val.parse::<f64>().map_err(|_| TextsError::InvalidNumber(val.to_string()))
    };

    let trigger = match on {
        "start" => Trigger::StartUp,
        "change" => {
            if sensor.is_empty() {
                return Err(TextsError::MissingField("sensor"));
            }
            Trigger::Change { sensor: sensor.to_string() }
        }
        "becomes" => {
            if sensor.is_empty() {
                return Err(TextsError::MissingField("sensor"));
            }
            let value = form::field(&pairs, "value").to_string();
            Trigger::Becomes { sensor: sensor.to_string(), value }
        }
        "step" => {
            if sensor.is_empty() {
                return Err(TextsError::MissingField("sensor"));
            }
            let step = parse_num("step")?;
            Trigger::Step { sensor: sensor.to_string(), step }
        }
        "below" => {
            if sensor.is_empty() { return Err(TextsError::MissingField("sensor")); }
            Trigger::Below { sensor: sensor.to_string(), threshold: parse_num("threshold")?, hysteresis: parse_num("hysteresis")? }
        }
        "above" => {
            if sensor.is_empty() { return Err(TextsError::MissingField("sensor")); }
            Trigger::Above { sensor: sensor.to_string(), threshold: parse_num("threshold")?, hysteresis: parse_num("hysteresis")? }
        }
        "at" => {
            let at_str = form::field(&pairs, "at");
            let (h_str, m_str) =
                at_str.split_once(':').ok_or_else(|| TextsError::InvalidTime(at_str.to_string()))?;
            let h: u16 = h_str.parse().map_err(|_| TextsError::InvalidTime(at_str.to_string()))?;
            let m: u16 = m_str.parse().map_err(|_| TextsError::InvalidTime(at_str.to_string()))?;
            if h >= 24 || m >= 60 {
                return Err(TextsError::InvalidTime(at_str.to_string()));
            }
            Trigger::At { minute: h * 60 + m }
        }
        "every" => {
            let ev_str = form::field(&pairs, "every");
            let seconds: u32 =
                ev_str.parse().map_err(|_| TextsError::InvalidNumber(ev_str.to_string()))?;
            Trigger::Every { seconds }
        }
        other => return Err(TextsError::UnknownTrigger(other.to_string())),
    };

    let conditions: Vec<String> = pairs
        .iter()
        .filter(|(k, _)| k == "if")
        .map(|(_, v)| v.clone())
        .collect();

    let do_field = form::field(&pairs, "do");
    let action = match do_field {
        "send" => Action::Send,
        "status" => Action::UpdateStatus,
        other => return Err(TextsError::UnknownAction(other.to_string())),
    };

    let text = form::field(&pairs, "text").to_string();
    let automation = Automation { trigger, conditions, action, text };

    autotext::automation::validate(&automation, &values::known).map_err(TextsError::Automation)?;

    template::validate(
        &automation.text,
        &|var| if values::known(var) { None } else { Some(TemplateError::Unknown) },
        has_template,
    )
    .map_err(TextsError::Template)?;

    library::check_calls("", &automation.text, library).map_err(TextsError::Call)?;
    Ok(automation)
}

/// Encodes a `StatusConfig` into form-urlencoded format.
pub fn status_config_to_form(config: &StatusConfig) -> String {
    let every_secs = (config.every_ms / 1000).to_string();
    let important_str = config.important.join(",");
    let bottom_str = if config.keep_at_bottom { "1" } else { "0" };
    form::encode(&[
        ("every", &every_secs),
        ("important", &important_str),
        ("bottom", bottom_str),
    ])
}

/// Parses and validates a `StatusConfig` from form-urlencoded format.
pub fn status_config_from_form(body: &str) -> Result<StatusConfig, TextsError> {
    let pairs = form::parse(body);
    let every_str = form::field(&pairs, "every");
    let secs: u64 = every_str.parse().map_err(|_| TextsError::StatusEvery)?;
    if !(10..=86400).contains(&secs) {
        return Err(TextsError::StatusEvery);
    }
    let every_ms = secs * 1000;

    let important_str = form::field(&pairs, "important");
    let important: Vec<String> = if important_str.is_empty() {
        Vec::new()
    } else {
        let list: Vec<String> =
            important_str.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        for item in &list {
            if !values::known(item) {
                return Err(TextsError::UnknownSensor(item.clone()));
            }
        }
        list
    };

    let bottom_str = form::field(&pairs, "bottom");
    let keep_at_bottom = match bottom_str {
        "1" | "true" => true,
        "0" | "false" => false,
        _ => return Err(TextsError::InvalidLiveBottom),
    };

    Ok(StatusConfig { every_ms, important, keep_at_bottom })
}

/// Encodes `Live` status state into form-urlencoded format.
pub fn live_to_form(live: &Live) -> String {
    let id_str = live.message_id.to_string();
    let snap_str = snapshot_to_str(&live.snapshot);
    let bottom_str = if live.at_bottom { "1" } else { "0" };
    form::encode(&[("id", &id_str), ("snap", &snap_str), ("bottom", bottom_str)])
}

/// Parses and restores `Live` status state from form-urlencoded format.
pub fn live_from_form(body: &str) -> Result<Live, TextsError> {
    let pairs = form::parse(body);
    let id_str = form::field(&pairs, "id");
    let message_id: i64 = id_str.parse().map_err(|_| TextsError::InvalidLiveId)?;

    let snap_str = form::field(&pairs, "snap");
    let snapshot = str_to_snapshot(snap_str)?;

    let bottom_str = form::field(&pairs, "bottom");
    let at_bottom = match bottom_str {
        "1" | "true" => true,
        "0" | "false" => false,
        _ => return Err(TextsError::InvalidLiveBottom),
    };

    Ok(Live { message_id, snapshot, updated_ms: 0, at_bottom })
}

fn snapshot_to_str(snap: &[Option<String>]) -> String {
    snap.iter()
        .map(|item| match item {
            None => "-".to_string(),
            Some(s) => format!("+{}", s.replace('\\', "\\\\").replace(',', "\\,")),
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn str_to_snapshot(s: &str) -> Result<Vec<Option<String>>, TextsError> {
    if s.is_empty() {
        return Ok(Vec::new());
    }
    let mut items = Vec::new();
    let mut current = String::new();
    let mut escaped = false;
    for ch in s.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == ',' {
            items.push(parse_snap_item(&current)?);
            current.clear();
        } else {
            current.push(ch);
        }
    }
    items.push(parse_snap_item(&current)?);
    Ok(items)
}

fn parse_snap_item(s: &str) -> Result<Option<String>, TextsError> {
    if s == "-" {
        Ok(None)
    } else if let Some(val) = s.strip_prefix('+') {
        Ok(Some(val.to_string()))
    } else {
        Err(TextsError::InvalidNumber(format!("некоректний елемент знімку: {s}")))
    }
}

#[cfg(test)]
mod tests;
