//! Device settings. A `Config` can only be built through `from_form`, so every
//! value the firmware sees has already been validated.

use std::fmt;

use crate::form;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    ssid: String,
    wifi_password: String,
    bot_token: String,
    chat_id: i64,
    device_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigError {
    Ssid,
    WifiPassword,
    BotToken,
    ChatId,
    DeviceName,
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ConfigError::Ssid => "Назва Wi-Fi мережі має бути від 1 до 32 байт.",
            ConfigError::WifiPassword => "Пароль Wi-Fi має бути порожнім або від 8 до 63 символів.",
            ConfigError::BotToken => "Токен бота має вигляд 123456789:AAAA… (його дає @BotFather).",
            ConfigError::ChatId => "Chat ID має бути цілим числом, наприклад 123456789 або -100….",
            ConfigError::DeviceName => "Назва пристрою має бути від 1 до 32 символів.",
        })
    }
}

impl Config {
    /// Builds a config from the settings form, or from the stored copy when
    /// `existing` is `None`.
    ///
    /// The page never shows secrets, so an empty token keeps the existing one,
    /// and an empty Wi-Fi password keeps the existing one while the network
    /// stays the same (for a new network it means an open one).
    pub fn from_form(body: &str, existing: Option<&Config>) -> Result<Config, ConfigError> {
        let fields = form::parse(body);
        let field = |name: &str| {
            fields.iter().find(|(key, _)| key == name).map_or("", |(_, value)| value.as_str())
        };
        let ssid = parse_ssid(field("ssid"))?;
        let same_network = existing.filter(|old| old.ssid == ssid);
        Ok(Config {
            wifi_password: parse_wifi_password(field("password"), same_network)?,
            bot_token: parse_bot_token(field("token").trim(), existing)?,
            chat_id: field("chat_id").trim().parse().map_err(|_| ConfigError::ChatId)?,
            device_name: parse_device_name(field("name").trim())?,
            ssid,
        })
    }

    /// The stored representation; `from_form(&c.to_form(), None)` gives `c` back.
    pub fn to_form(&self) -> String {
        form::encode(&[
            ("ssid", &self.ssid),
            ("password", &self.wifi_password),
            ("token", &self.bot_token),
            ("chat_id", &self.chat_id.to_string()),
            ("name", &self.device_name),
        ])
    }

    pub fn ssid(&self) -> &str {
        &self.ssid
    }

    pub fn wifi_password(&self) -> &str {
        &self.wifi_password
    }

    pub fn bot_token(&self) -> &str {
        &self.bot_token
    }

    pub fn chat_id(&self) -> i64 {
        self.chat_id
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }
}

fn parse_ssid(ssid: &str) -> Result<String, ConfigError> {
    match ssid.len() {
        1..=32 => Ok(ssid.to_string()),
        _ => Err(ConfigError::Ssid),
    }
}

fn parse_wifi_password(password: &str, same_network: Option<&Config>) -> Result<String, ConfigError> {
    match (password.len(), same_network) {
        (0, Some(old)) => Ok(old.wifi_password.clone()),
        (0 | 8..=63, _) => Ok(password.to_string()),
        _ => Err(ConfigError::WifiPassword),
    }
}

/// Telegram tokens look like `123456789:AAH…` with a 35-character secret.
fn parse_bot_token(token: &str, existing: Option<&Config>) -> Result<String, ConfigError> {
    if let (true, Some(old)) = (token.is_empty(), existing) {
        return Ok(old.bot_token.clone());
    }
    let (id, secret) = token.split_once(':').ok_or(ConfigError::BotToken)?;
    let id_ok = !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit());
    let secret_ok = secret.len() >= 30
        && secret.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if id_ok && secret_ok {
        Ok(token.to_string())
    } else {
        Err(ConfigError::BotToken)
    }
}

fn parse_device_name(name: &str) -> Result<String, ConfigError> {
    match name.chars().count() {
        1..=32 => Ok(name.to_string()),
        _ => Err(ConfigError::DeviceName),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";

    fn body(ssid: &str, password: &str, token: &str, chat_id: &str, name: &str) -> String {
        form::encode(&[
            ("ssid", ssid),
            ("password", password),
            ("token", token),
            ("chat_id", chat_id),
            ("name", name),
        ])
    }

    fn valid() -> Config {
        Config::from_form(&body("Home", "secret123", TOKEN, "-1001234", "Дім"), None).unwrap()
    }

    #[test]
    fn accepts_a_complete_form() {
        let c = valid();
        assert_eq!(c.ssid(), "Home");
        assert_eq!(c.wifi_password(), "secret123");
        assert_eq!(c.bot_token(), TOKEN);
        assert_eq!(c.chat_id(), -1001234);
        assert_eq!(c.device_name(), "Дім");
    }

    #[test]
    fn trims_everything_but_the_ssid_and_password() {
        let c = Config::from_form(
            &body(" My Net ", " pass word ", &format!(" {TOKEN} "), " 42 ", " Дача "),
            None,
        )
        .unwrap();
        assert_eq!(c.ssid(), " My Net ");
        assert_eq!(c.wifi_password(), " pass word ");
        assert_eq!(c.bot_token(), TOKEN);
        assert_eq!(c.chat_id(), 42);
        assert_eq!(c.device_name(), "Дача");
    }

    #[test]
    fn rejects_bad_ssid() {
        let error = Err(ConfigError::Ssid);
        assert_eq!(Config::from_form(&body("", "", TOKEN, "1", "x"), None), error);
        assert_eq!(Config::from_form(&body(&"s".repeat(33), "", TOKEN, "1", "x"), None), error);
    }

    #[test]
    fn rejects_bad_wifi_password() {
        let error = Err(ConfigError::WifiPassword);
        assert_eq!(Config::from_form(&body("s", "1234567", TOKEN, "1", "x"), None), error);
        assert_eq!(Config::from_form(&body("s", &"p".repeat(64), TOKEN, "1", "x"), None), error);
    }

    #[test]
    fn rejects_bad_bot_token() {
        for token in ["", "abc:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw", "123456789:short", "123:AAH dq"] {
            assert_eq!(
                Config::from_form(&body("s", "", token, "1", "x"), None),
                Err(ConfigError::BotToken),
                "{token}"
            );
        }
    }

    #[test]
    fn rejects_bad_chat_id() {
        for chat_id in ["", "abc", "12.5", "99999999999999999999"] {
            assert_eq!(
                Config::from_form(&body("s", "", TOKEN, chat_id, "x"), None),
                Err(ConfigError::ChatId),
                "{chat_id}"
            );
        }
    }

    #[test]
    fn rejects_bad_device_name() {
        let error = Err(ConfigError::DeviceName);
        assert_eq!(Config::from_form(&body("s", "", TOKEN, "1", "  "), None), error);
        assert_eq!(Config::from_form(&body("s", "", TOKEN, "1", &"я".repeat(33)), None), error);
    }

    #[test]
    fn empty_secrets_keep_the_existing_ones_on_the_same_network() {
        let old = valid();
        let c = Config::from_form(&body("Home", "", "", "7", "Нова"), Some(&old)).unwrap();
        assert_eq!(c.wifi_password(), "secret123");
        assert_eq!(c.bot_token(), TOKEN);
        assert_eq!(c.chat_id(), 7);
        assert_eq!(c.device_name(), "Нова");
    }

    #[test]
    fn empty_password_on_a_new_network_means_an_open_network() {
        let old = valid();
        let c = Config::from_form(&body("Cafe", "", "", "7", "x"), Some(&old)).unwrap();
        assert_eq!(c.wifi_password(), "");
    }

    #[test]
    fn stored_form_round_trips() {
        let c = valid();
        assert_eq!(Config::from_form(&c.to_form(), None), Ok(c));
    }
}
