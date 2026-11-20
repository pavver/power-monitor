//! The device name.

use serde::Serialize;

use super::SettingsError;
use crate::form;

const DEFAULT_DEVICE_NAME: &str = "Дім";

/// Device name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Messages {
    device_name: String,
}

impl Default for Messages {
    fn default() -> Self {
        Self {
            device_name: DEFAULT_DEVICE_NAME.to_string(),
        }
    }
}

impl Messages {
    /// Changes the field named by `query` (`field=<key>`, the key being
    /// "device_name") to `value`, the text as it came; returns the key and
    /// the value as it is to be stored (same as `fields()` would give).
    pub fn set(&mut self, query: &str, value: &str) -> Result<(&'static str, String), SettingsError> {
        let fields = form::parse(query);
        let field = form::field(&fields, "field");
        if field == "device_name" {
            let name = device_name(value).ok_or(SettingsError::DeviceName)?;
            self.device_name = name.clone();
            return Ok(("device_name", name));
        }
        Err(SettingsError::UnknownField)
    }

    /// Reads `device_name`, as the page sends it.
    pub fn from_form(body: &str) -> Result<Self, SettingsError> {
        let fields = form::parse(body);
        let device_name = device_name(form::field(&fields, "device_name")).ok_or(SettingsError::DeviceName)?;
        Ok(Self { device_name })
    }

    /// What `fields` stored, `get` giving each by its key; a missing or
    /// broken one is the default.
    pub fn from_stored(get: impl Fn(&str) -> Option<String>) -> Self {
        let device_name = get("device_name").as_deref().and_then(device_name).unwrap_or_else(|| DEFAULT_DEVICE_NAME.to_string());
        Self { device_name }
    }

    /// Every value with its key, for storing.
    pub fn fields(&self) -> Vec<(&'static str, String)> {
        vec![("device_name", self.device_name.clone())]
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }
}

/// A device name of 1 to 32 characters, trimmed.
fn device_name(value: &str) -> Option<String> {
    let name = value.trim();
    (1..=32).contains(&name.chars().count()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn messages_form(overrides: &[(&str, &str)]) -> String {
        let mut fields = Messages::default().fields();
        for (key, value) in overrides {
            if let Some(entry) = fields.iter_mut().find(|(k, _)| k == key) {
                entry.1 = value.to_string();
            } else {
                fields.push((key, value.to_string()));
            }
        }
        encode(&fields)
    }

    fn encode(fields: &[(&str, String)]) -> String {
        form::encode(&fields.iter().map(|(k, v)| (*k, v.as_str())).collect::<Vec<_>>())
    }

    #[test]
    fn messages_read_device_name() {
        let m = Messages::from_form(&messages_form(&[("device_name", " Дача ")])).unwrap();
        assert_eq!(m.device_name(), "Дача");
        assert_eq!(Messages::from_form(&encode(&m.fields())), Ok(m));
    }

    #[test]
    fn messages_reject_bad_device_name() {
        for name in ["  ", &"я".repeat(33)] {
            assert_eq!(
                Messages::from_form(&messages_form(&[("device_name", name)])),
                Err(SettingsError::DeviceName)
            );
        }
    }

    #[test]
    fn stored_device_name_fills_missing_or_broken_with_defaults() {
        let stored = [("device_name", "Дача")];
        let m = Messages::from_stored(|key| stored.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string()));
        assert_eq!(m.device_name(), "Дача");
        assert_eq!(Messages::from_stored(|_| None), Messages::default());
        assert_eq!(Messages::from_stored(|_| Some("   ".to_string())), Messages::default());
    }

    #[test]
    fn stored_device_name_round_trips() {
        let m = Messages::from_form(&messages_form(&[("device_name", "Офіс")])).unwrap();
        let stored = m.fields();
        let back = Messages::from_stored(|key| stored.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string()));
        assert_eq!(back, m);
    }

    #[test]
    fn setting_device_name_is_trimmed_and_bad_name_leaves_messages_unchanged() {
        let mut m = Messages::default();
        let res = m.set("field=device_name", " Дача ").unwrap();
        assert_eq!(res, ("device_name", "Дача".to_string()));
        assert_eq!(m.device_name(), "Дача");

        let before = m.clone();
        for bad in ["", "   ", &"я".repeat(33)] {
            assert_eq!(m.set("field=device_name", bad), Err(SettingsError::DeviceName), "{bad}");
            assert_eq!(m, before);
        }
    }

    #[test]
    fn setting_unknown_field_errors_and_leaves_messages_unchanged() {
        let mut m = Messages::default();
        let before = m.clone();
        assert_eq!(m.set("field=unknown", "test"), Err(SettingsError::UnknownField));
        assert_eq!(m.set("field=", "test"), Err(SettingsError::UnknownField));
        assert_eq!(m.set("other=123", ""), Err(SettingsError::UnknownField));
        assert_eq!(m, before);
    }
}
