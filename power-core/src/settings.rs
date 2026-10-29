//! Device settings, stored as three independent parts. Each part can only be
//! built by parsing, so every value the firmware holds is already valid.

use std::fmt;

use serde::Serialize;

use crate::form;
use crate::template::{self, TemplateError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsError {
    Ssid,
    WifiPassword,
    BotToken,
    ChatId,
    DeviceName,
    Text(Text, TemplateError),
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Ssid => f.write_str("Назва Wi-Fi мережі має бути від 1 до 32 байт."),
            SettingsError::WifiPassword => {
                f.write_str("Пароль Wi-Fi має бути порожнім або від 8 до 63 символів.")
            }
            SettingsError::BotToken => {
                f.write_str("Токен бота має вигляд 123456789:AAH… — скопіюйте його з @BotFather.")
            }
            SettingsError::ChatId => f.write_str("Chat ID має бути цілим числом."),
            SettingsError::DeviceName => f.write_str("Назва пристрою має бути від 1 до 32 символів."),
            SettingsError::Text(text, error) => write!(f, "{}: {error}.", text.label()),
        }
    }
}

/// The network the device joins as a station.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WifiCredentials {
    ssid: String,
    password: String,
}

impl WifiCredentials {
    pub fn parse(ssid: &str, password: &str) -> Result<Self, SettingsError> {
        if !(1..=32).contains(&ssid.len()) {
            return Err(SettingsError::Ssid);
        }
        if !matches!(password.len(), 0 | 8..=63) {
            return Err(SettingsError::WifiPassword);
        }
        Ok(Self { ssid: ssid.to_string(), password: password.to_string() })
    }

    /// Reads the `ssid` and `password` fields.
    pub fn from_form(body: &str) -> Result<Self, SettingsError> {
        let fields = form::parse(body);
        Self::parse(form::field(&fields, "ssid"), form::field(&fields, "password"))
    }

    pub fn to_form(&self) -> String {
        form::encode(&[("ssid", &self.ssid), ("password", &self.password)])
    }

    pub fn ssid(&self) -> &str {
        &self.ssid
    }

    pub fn password(&self) -> &str {
        &self.password
    }
}

/// A Telegram bot token, `123456789:AAH…`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BotToken(String);

impl BotToken {
    pub fn parse(token: &str) -> Result<Self, SettingsError> {
        let token = token.trim();
        let (id, secret) = token.split_once(':').ok_or(SettingsError::BotToken)?;
        let id_ok = !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit());
        let secret_ok = secret.len() >= 30
            && secret.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
        if id_ok && secret_ok {
            Ok(Self(token.to_string()))
        } else {
            Err(SettingsError::BotToken)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The bot and the chat it reports to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bot {
    token: BotToken,
    chat_id: i64,
}

impl Bot {
    /// Reads the `token` and `chat_id` fields.
    pub fn from_form(body: &str) -> Result<Self, SettingsError> {
        let fields = form::parse(body);
        Ok(Self {
            token: BotToken::parse(form::field(&fields, "token"))?,
            chat_id: form::field(&fields, "chat_id").trim().parse().map_err(|_| SettingsError::ChatId)?,
        })
    }

    pub fn to_form(&self) -> String {
        form::encode(&[("token", self.token.as_str()), ("chat_id", &self.chat_id.to_string())])
    }

    pub fn token(&self) -> &BotToken {
        &self.token
    }

    pub fn chat_id(&self) -> i64 {
        self.chat_id
    }
}

/// The editable texts, one per kind of message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text {
    StartPresent,
    StartAbsent,
    Lost,
    Returned,
}

impl Text {
    pub const ALL: [Text; 4] = [Text::StartPresent, Text::StartAbsent, Text::Lost, Text::Returned];

    /// Form field and JSON key.
    pub fn key(self) -> &'static str {
        match self {
            Text::StartPresent => "start_present",
            Text::StartAbsent => "start_absent",
            Text::Lost => "lost",
            Text::Returned => "returned",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Text::StartPresent => "Запуск, світло є",
            Text::StartAbsent => "Запуск, світла немає",
            Text::Lost => "Світло зникло",
            Text::Returned => "Світло з'явилось",
        }
    }

    /// Start-up messages have no previous state to measure.
    fn has_duration(self) -> bool {
        matches!(self, Text::Lost | Text::Returned)
    }

    fn default_template(self) -> &'static str {
        match self {
            Text::StartPresent => "🟢 {device}: монітор запущено, світло є.",
            Text::StartAbsent => "🔴 {device}: монітор запущено, світла немає.",
            Text::Lost => "🔴 {device}: світло зникло.\nБуло {duration}.",
            Text::Returned => "🟢 {device}: світло з'явилось.\nНе було {duration}.",
        }
    }
}

/// Device name and message templates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Messages {
    device_name: String,
    start_present: String,
    start_absent: String,
    lost: String,
    returned: String,
}

impl Default for Messages {
    fn default() -> Self {
        Self {
            device_name: "Дім".to_string(),
            start_present: Text::StartPresent.default_template().to_string(),
            start_absent: Text::StartAbsent.default_template().to_string(),
            lost: Text::Lost.default_template().to_string(),
            returned: Text::Returned.default_template().to_string(),
        }
    }
}

impl Messages {
    /// Reads `device_name` and one field per `Text::key`.
    pub fn from_form(body: &str) -> Result<Self, SettingsError> {
        let fields = form::parse(body);
        let device_name = form::field(&fields, "device_name").trim();
        if !(1..=32).contains(&device_name.chars().count()) {
            return Err(SettingsError::DeviceName);
        }
        let text = |text: Text| {
            let value = form::field(&fields, text.key());
            template::validate(value, text.has_duration())
                .map(|()| value.to_string())
                .map_err(|error| SettingsError::Text(text, error))
        };
        Ok(Self {
            device_name: device_name.to_string(),
            start_present: text(Text::StartPresent)?,
            start_absent: text(Text::StartAbsent)?,
            lost: text(Text::Lost)?,
            returned: text(Text::Returned)?,
        })
    }

    pub fn to_form(&self) -> String {
        let mut pairs = vec![("device_name", self.device_name.as_str())];
        pairs.extend(Text::ALL.map(|text| (text.key(), self.template(text))));
        form::encode(&pairs)
    }

    pub fn device_name(&self) -> &str {
        &self.device_name
    }

    pub fn template(&self, text: Text) -> &str {
        match text {
            Text::StartPresent => &self.start_present,
            Text::StartAbsent => &self.start_absent,
            Text::Lost => &self.lost,
            Text::Returned => &self.returned,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "123456789:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw";

    fn messages_form(overrides: &[(&str, &str)]) -> String {
        let defaults = Messages::default();
        let mut fields: Vec<(&str, String)> = vec![("device_name", defaults.device_name.clone())];
        fields.extend(Text::ALL.map(|t| (t.key(), defaults.template(t).to_string())));
        for (key, value) in overrides {
            fields.iter_mut().find(|(k, _)| k == key).unwrap().1 = value.to_string();
        }
        let pairs: Vec<(&str, &str)> = fields.iter().map(|(k, v)| (*k, v.as_str())).collect();
        form::encode(&pairs)
    }

    #[test]
    fn wifi_accepts_valid_credentials_without_trimming() {
        let wifi = WifiCredentials::parse(" My Net ", " pass word ").unwrap();
        assert_eq!((wifi.ssid(), wifi.password()), (" My Net ", " pass word "));
        assert!(WifiCredentials::parse("Open", "").is_ok());
    }

    #[test]
    fn wifi_rejects_bad_ssid() {
        assert_eq!(WifiCredentials::parse("", ""), Err(SettingsError::Ssid));
        assert_eq!(WifiCredentials::parse(&"s".repeat(33), ""), Err(SettingsError::Ssid));
    }

    #[test]
    fn wifi_rejects_bad_password() {
        assert_eq!(WifiCredentials::parse("s", "1234567"), Err(SettingsError::WifiPassword));
        assert_eq!(WifiCredentials::parse("s", &"p".repeat(64)), Err(SettingsError::WifiPassword));
    }

    #[test]
    fn wifi_round_trips_through_its_form() {
        let wifi = WifiCredentials::parse("Дім & сад", "pa=ss+word").unwrap();
        assert_eq!(WifiCredentials::from_form(&wifi.to_form()), Ok(wifi));
    }

    #[test]
    fn token_is_trimmed_and_checked() {
        assert_eq!(BotToken::parse(&format!(" {TOKEN} ")).unwrap().as_str(), TOKEN);
        for bad in ["", "abc:AAHdqTcvCH1vGWJxfSeofSAs0K5PALDsaw", "123456789:short", "123:AAH dq"] {
            assert_eq!(BotToken::parse(bad), Err(SettingsError::BotToken), "{bad}");
        }
    }

    #[test]
    fn bot_reads_token_and_chat_id() {
        let bot = Bot::from_form(&form::encode(&[("token", TOKEN), ("chat_id", " -1001234 ")])).unwrap();
        assert_eq!((bot.token().as_str(), bot.chat_id()), (TOKEN, -1001234));
        assert_eq!(Bot::from_form(&bot.to_form()), Ok(bot));
    }

    #[test]
    fn bot_rejects_bad_chat_id() {
        for chat_id in ["", "abc", "12.5", "99999999999999999999"] {
            let body = form::encode(&[("token", TOKEN), ("chat_id", chat_id)]);
            assert_eq!(Bot::from_form(&body), Err(SettingsError::ChatId), "{chat_id}");
        }
    }

    #[test]
    fn messages_read_every_text() {
        let m = Messages::from_form(&messages_form(&[("device_name", " Дача "), ("lost", "Нема {duration}")]))
            .unwrap();
        assert_eq!(m.device_name(), "Дача");
        assert_eq!(m.template(Text::Lost), "Нема {duration}");
        assert_eq!(m.template(Text::Returned), Text::Returned.default_template());
        assert_eq!(Messages::from_form(&m.to_form()), Ok(m));
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
    fn messages_reject_bad_templates_naming_the_text() {
        assert_eq!(
            Messages::from_form(&messages_form(&[("start_absent", "{duration}")])),
            Err(SettingsError::Text(Text::StartAbsent, TemplateError::NoDuration))
        );
        assert_eq!(
            Messages::from_form(&messages_form(&[("returned", "")])),
            Err(SettingsError::Text(Text::Returned, TemplateError::Empty))
        );
    }

    #[test]
    fn default_templates_are_valid() {
        assert_eq!(Messages::from_form(&messages_form(&[])), Ok(Messages::default()));
    }

    #[test]
    fn errors_explain_which_text_is_wrong() {
        let error = SettingsError::Text(Text::Lost, TemplateError::Empty);
        assert_eq!(error.to_string(), "Світло зникло: текст не може бути порожнім.");
    }
}
