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
    ThreadId,
    DeviceName,
    Text(Text, TemplateError),
    BmsAddress,
    Divider,
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
            SettingsError::ThreadId => f.write_str("Номер гілки форуму має бути додатним цілим числом."),
            SettingsError::DeviceName => f.write_str("Назва пристрою має бути від 1 до 32 символів."),
            SettingsError::Text(text, error) => write!(f, "{}: {error}.", text.label()),
            SettingsError::BmsAddress => f.write_str("Адреса BMS має вигляд AA:BB:CC:DD:EE:FF."),
            SettingsError::Divider => {
                f.write_str("Опір резисторів — ціле число ом від 1 до 100 000 000, наприклад 330000 і 10000.")
            }
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

/// The JK BMS to read over Bluetooth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Battery {
    /// Most significant byte first, as printed.
    pub address: [u8; 6],
    /// Random (not public) Bluetooth address.
    pub random: bool,
    /// As advertised, for the page.
    pub name: String,
}

impl Battery {
    /// Reads `address` (`AA:BB:CC:DD:EE:FF`), `random` (`1` when set) and `name`.
    pub fn from_form(body: &str) -> Result<Self, SettingsError> {
        let fields = form::parse(body);
        let parts: Vec<&str> = form::field(&fields, "address").split(':').collect();
        let mut address = [0; 6];
        if parts.len() != address.len() {
            return Err(SettingsError::BmsAddress);
        }
        for (byte, part) in address.iter_mut().zip(parts) {
            if part.len() != 2 {
                return Err(SettingsError::BmsAddress);
            }
            *byte = u8::from_str_radix(part, 16).map_err(|_| SettingsError::BmsAddress)?;
        }
        Ok(Self {
            address,
            random: form::field(&fields, "random") == "1",
            name: form::field(&fields, "name").chars().take(32).collect(),
        })
    }

    pub fn to_form(&self) -> String {
        let random = if self.random { "1" } else { "" };
        form::encode(&[("address", &self.address_text()), ("random", random), ("name", &self.name)])
    }

    pub fn address_text(&self) -> String {
        self.address.map(|b| format!("{b:02X}")).join(":")
    }
}

/// The resistors that bring the battery voltage down to the ADC pin:
/// battery → `top` → pin → `bottom` → GND.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Divider {
    top_ohm: u32,
    bottom_ohm: u32,
}

impl Divider {
    const MAX_OHM: u32 = 100_000_000;
    /// R1 330k and R2 10k from sheet 1 of the schematic: up to 80 V.
    pub const SHEET_1: Self = Self { top_ohm: 330_000, bottom_ohm: 10_000 };

    /// Reads `top` and `bottom`, whole ohms.
    pub fn from_form(body: &str) -> Result<Self, SettingsError> {
        let fields = form::parse(body);
        let ohm = |name| match form::field(&fields, name).trim().parse() {
            Ok(ohm @ 1..=Self::MAX_OHM) => Ok(ohm),
            _ => Err(SettingsError::Divider),
        };
        Ok(Self { top_ohm: ohm("top")?, bottom_ohm: ohm("bottom")? })
    }

    pub fn to_form(&self) -> String {
        form::encode(&[("top", &self.top_ohm.to_string()), ("bottom", &self.bottom_ohm.to_string())])
    }

    /// The divider a firmware build brings, from `DIVIDER_TOP_OHM` and
    /// `DIVIDER_BOTTOM_OHM`: both, or neither for the one on sheet 1.
    pub fn built_in(top: Option<&str>, bottom: Option<&str>) -> Result<Self, &'static str> {
        match (top, bottom) {
            (None, None) => Ok(Self::SHEET_1),
            (Some(top), Some(bottom)) => Self::from_form(&form::encode(&[("top", top), ("bottom", bottom)]))
                .map_err(|_| "DIVIDER_TOP_OHM і DIVIDER_BOTTOM_OHM — цілі оми від 1 до 100000000"),
            _ => Err("задайте обидва: DIVIDER_TOP_OHM і DIVIDER_BOTTOM_OHM"),
        }
    }

    /// How a choice made on the page is kept; switching the divider off is
    /// kept too, so the built-in one does not come back after a restart.
    pub fn to_stored(divider: Option<Self>) -> String {
        divider.map_or_else(String::new, |divider| divider.to_form())
    }

    /// The divider to use given what was kept (`None`: never chosen).
    pub fn from_stored(stored: Option<&str>, built_in: Self) -> Option<Self> {
        match stored {
            None => Some(built_in),
            Some(stored) => Self::from_form(stored).ok(),
        }
    }

    /// The battery voltage that puts `pin_mv` on the ADC pin.
    pub fn battery_mv(&self, pin_mv: u32) -> u32 {
        let total = u64::from(self.top_ohm) + u64::from(self.bottom_ohm);
        let mv = u64::from(pin_mv) * total / u64::from(self.bottom_ohm);
        u32::try_from(mv).unwrap_or(u32::MAX)
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

/// Where reports go: a chat, and in a forum group optionally one topic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Destination {
    pub chat_id: i64,
    pub thread_id: Option<i64>,
}

/// The bot and the chat it reports to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bot {
    token: BotToken,
    to: Destination,
}

impl Bot {
    /// Reads the `token`, `chat_id` and optional `thread_id` fields.
    pub fn from_form(body: &str) -> Result<Self, SettingsError> {
        let fields = form::parse(body);
        let thread_id = match form::field(&fields, "thread_id").trim() {
            "" => None,
            text => Some(text.parse().ok().filter(|id: &i64| *id > 0).ok_or(SettingsError::ThreadId)?),
        };
        Ok(Self {
            token: BotToken::parse(form::field(&fields, "token"))?,
            to: Destination {
                chat_id: form::field(&fields, "chat_id").trim().parse().map_err(|_| SettingsError::ChatId)?,
                thread_id,
            },
        })
    }

    pub fn to_form(&self) -> String {
        let chat_id = self.to.chat_id.to_string();
        let thread_id = self.to.thread_id.map(|id| id.to_string()).unwrap_or_default();
        form::encode(&[("token", self.token.as_str()), ("chat_id", &chat_id), ("thread_id", &thread_id)])
    }

    pub fn token(&self) -> &BotToken {
        &self.token
    }

    pub fn destination(&self) -> Destination {
        self.to
    }
}

/// The editable texts, one per kind of message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text {
    StartPresent,
    StartAbsent,
    Lost,
    Returned,
    Address,
    Test,
}

/// The note on a power change delivered late. Devices before `{delay}`
/// appended it themselves, so their saved texts get it on migration.
macro_rules! delay_note {
    () => {
        "{delay?\n(сталося {delay} тому)}"
    };
}

impl Text {
    pub const ALL: [Text; 6] = [Text::StartPresent, Text::StartAbsent, Text::Lost, Text::Returned, Text::Address, Text::Test];

    /// Form field, JSON key and settings key (at most 15 bytes).
    pub fn key(self) -> &'static str {
        match self {
            Text::StartPresent => "start_present",
            Text::StartAbsent => "start_absent",
            Text::Lost => "lost",
            Text::Returned => "returned",
            Text::Address => "address",
            Text::Test => "test",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Text::StartPresent => "Запуск, світло є",
            Text::StartAbsent => "Запуск, світла немає",
            Text::Lost => "Світло зникло",
            Text::Returned => "Світло з'явилось",
            Text::Address => "Нова адреса панелі",
            Text::Test => "Тестове повідомлення",
        }
    }

    /// The values of `template::OWN` this text has. Start-up messages have
    /// no previous state to measure and are never late.
    pub(crate) fn own(self) -> &'static [&'static str] {
        match self {
            Text::Lost | Text::Returned => &["duration", "delay"],
            Text::Address => &["ip"],
            Text::StartPresent | Text::StartAbsent | Text::Test => &[],
        }
    }

    fn default_template(self) -> &'static str {
        // The last line: the BMS if it answered, else the divider, else nothing.
        macro_rules! battery {
            () => {
                "{bms_soc?\n🔋 {bms_soc}% · {bms_voltage} В · {bms_energy} кВт·год}{!bms_soc?{battery_voltage?\n🔋 {battery_voltage} В}}"
            };
        }
        match self {
            Text::StartPresent => concat!("⚡ {device}: монітор запущено, світло є.", battery!()),
            Text::StartAbsent => concat!("🔌 {device}: монітор запущено, світла немає.", battery!()),
            Text::Lost => concat!("🔌 {device}: світло зникло.{duration?\nБуло {duration}.}", delay_note!(), battery!()),
            Text::Returned => concat!("⚡ {device}: світло з'явилось.{duration?\nНе було {duration}.}", delay_note!(), battery!()),
            Text::Address => "🌐 {device}: нова адреса панелі http://{ip}/",
            Text::Test => concat!("🔔 {device}: тестове повідомлення, бот працює.", battery!()),
        }
    }

    /// A text as it will be kept: blank turns the message off, except the
    /// test one, which is what checks the bot.
    fn parse(self, value: &str) -> Result<String, TemplateError> {
        match (value.trim().is_empty(), self) {
            (true, Text::Test) => Err(TemplateError::Blank),
            (true, _) => Ok(String::new()),
            (false, _) => template::validate(value, self.own()).map(|()| value.to_string()),
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
    address: String,
    test: String,
}

const DEFAULT_DEVICE_NAME: &str = "Дім";

impl Default for Messages {
    fn default() -> Self {
        Self::build(DEFAULT_DEVICE_NAME.to_string(), |text| Ok::<_, ()>(text.default_template().to_string())).unwrap()
    }
}

impl Messages {
    /// Reads `device_name` and one field per `Text::key`, as the page sends them.
    pub fn from_form(body: &str) -> Result<Self, SettingsError> {
        let fields = form::parse(body);
        let device_name = device_name(form::field(&fields, "device_name")).ok_or(SettingsError::DeviceName)?;
        Self::build(device_name, |text| text.parse(form::field(&fields, text.key())).map_err(|error| SettingsError::Text(text, error)))
    }

    /// What `fields` stored, `get` giving each by its key; a missing or
    /// broken one, e.g. a text from a newer firmware, is the default.
    pub fn from_stored(get: impl Fn(&str) -> Option<String>) -> Self {
        let device_name = get("device_name").as_deref().and_then(device_name).unwrap_or(DEFAULT_DEVICE_NAME.to_string());
        let text = |text: Text| get(text.key()).and_then(|value| text.parse(&value).ok());
        Self::build(device_name, |t| Ok::<_, ()>(text(t).unwrap_or(t.default_template().to_string()))).unwrap()
    }

    /// The single form firmware 0.1.0 stored all texts in.
    pub fn from_old_form(body: &str) -> Self {
        let fields = form::parse(body);
        let mut messages = Self::from_stored(|key| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone()));
        for text in [&mut messages.lost, &mut messages.returned] {
            if !text.is_empty() {
                text.push_str(delay_note!());
            }
        }
        messages
    }

    /// Every value with its key, for storing.
    pub fn fields(&self) -> Vec<(&'static str, &str)> {
        let mut fields = vec![("device_name", self.device_name.as_str())];
        fields.extend(Text::ALL.map(|text| (text.key(), self.template(text))));
        fields
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
            Text::Address => &self.address,
            Text::Test => &self.test,
        }
    }

    fn build<E>(device_name: String, mut text: impl FnMut(Text) -> Result<String, E>) -> Result<Self, E> {
        Ok(Self {
            device_name,
            start_present: text(Text::StartPresent)?,
            start_absent: text(Text::StartAbsent)?,
            lost: text(Text::Lost)?,
            returned: text(Text::Returned)?,
            address: text(Text::Address)?,
            test: text(Text::Test)?,
        })
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
        assert_eq!((bot.token().as_str(), bot.destination().chat_id), (TOKEN, -1001234));
        assert_eq!(Bot::from_form(&bot.to_form()), Ok(bot));
    }

    #[test]
    fn bot_reads_an_optional_forum_topic() {
        let body = form::encode(&[("token", TOKEN), ("chat_id", "-1001234"), ("thread_id", " 77 ")]);
        let bot = Bot::from_form(&body).unwrap();
        assert_eq!(bot.destination(), Destination { chat_id: -1001234, thread_id: Some(77) });
        assert_eq!(Bot::from_form(&bot.to_form()), Ok(bot));
        // Saved before topics existed, or a chat without them.
        let plain = Bot::from_form(&form::encode(&[("token", TOKEN), ("chat_id", "42")])).unwrap();
        assert_eq!(plain.destination(), Destination { chat_id: 42, thread_id: None });
        assert_eq!(Bot::from_form(&plain.to_form()), Ok(plain));
    }

    #[test]
    fn bot_rejects_bad_thread_id() {
        for thread_id in ["abc", "0", "-5", "1.5"] {
            let body = form::encode(&[("token", TOKEN), ("chat_id", "-100"), ("thread_id", thread_id)]);
            assert_eq!(Bot::from_form(&body), Err(SettingsError::ThreadId), "{thread_id}");
        }
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
        assert_eq!(Messages::from_form(&form::encode(&m.fields())), Ok(m));
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
            Err(SettingsError::Text(Text::StartAbsent, TemplateError::NotHere("duration")))
        );
        assert_eq!(
            Messages::from_form(&messages_form(&[("start_present", "{ip}")])),
            Err(SettingsError::Text(Text::StartPresent, TemplateError::NotHere("ip")))
        );
    }

    #[test]
    fn each_text_has_its_own_values() {
        let m = Messages::from_form(&messages_form(&[
            ("address", "{device}: http://{ip}/"),
            ("lost", "{delay?(сталося {delay} тому)}"),
            ("test", "{device}: {bms_soc}%"),
        ]))
        .unwrap();
        assert_eq!(m.template(Text::Address), "{device}: http://{ip}/");
        assert_eq!(m.template(Text::Lost), "{delay?(сталося {delay} тому)}");
        assert_eq!(m.template(Text::Test), "{device}: {bms_soc}%");
    }

    #[test]
    fn the_test_text_cannot_be_blank() {
        // It is what checks the bot; the address message may be turned off.
        assert_eq!(
            Messages::from_form(&messages_form(&[("test", " ")])),
            Err(SettingsError::Text(Text::Test, TemplateError::Blank))
        );
        assert_eq!(Messages::from_form(&messages_form(&[("address", "")])).unwrap().template(Text::Address), "");
    }

    #[test]
    fn stored_texts_fill_what_is_missing_or_broken_with_defaults() {
        let stored = [("device_name", "Дача"), ("lost", "Нема"), ("returned", "{nope}"), ("address", "")];
        let m = Messages::from_stored(|key| stored.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string()));
        assert_eq!(m.device_name(), "Дача");
        assert_eq!(m.template(Text::Lost), "Нема");
        assert_eq!(m.template(Text::Returned), Text::Returned.default_template());
        assert_eq!(m.template(Text::Address), "");
        assert_eq!(m.template(Text::Test), Text::Test.default_template());
        assert_eq!(Messages::from_stored(|_| None), Messages::default());
    }

    #[test]
    fn stored_texts_round_trip() {
        let m = Messages::from_form(&messages_form(&[("lost", "Нема & {duration}"), ("address", "")])).unwrap();
        let stored = m.fields();
        let back = Messages::from_stored(|key| stored.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string()));
        assert_eq!(back, m);
    }

    #[test]
    fn texts_saved_before_the_delay_value_keep_their_late_note() {
        // Those devices appended the note to every power change themselves.
        let old = form::encode(&[
            ("device_name", "Дача"),
            ("start_present", "Є"),
            ("start_absent", "Нема"),
            ("lost", "Зникло"),
            ("returned", ""),
        ]);
        let m = Messages::from_old_form(&old);
        assert_eq!(m.template(Text::Lost), "Зникло{delay?\n(сталося {delay} тому)}");
        assert_eq!(m.template(Text::Returned), "");
        assert_eq!(m.template(Text::StartPresent), "Є");
        assert_eq!(m.template(Text::Address), Text::Address.default_template());
        assert_eq!(m.template(Text::Test), Text::Test.default_template());
    }

    #[test]
    fn a_blank_text_is_kept_as_empty() {
        let m = Messages::from_form(&messages_form(&[("returned", " \n "), ("start_absent", "")])).unwrap();
        assert_eq!(m.template(Text::Returned), "");
        assert_eq!(m.template(Text::StartAbsent), "");
        assert_eq!(Messages::from_form(&form::encode(&m.fields())), Ok(m));
    }

    #[test]
    fn default_templates_are_valid() {
        assert_eq!(Messages::from_form(&messages_form(&[])), Ok(Messages::default()));
    }

    #[test]
    fn errors_explain_which_text_is_wrong() {
        let error = SettingsError::Text(Text::Lost, TemplateError::TooLong);
        assert_eq!(error.to_string(), "Світло зникло: текст довший за 500 символів.");
        let error = SettingsError::Text(Text::Test, TemplateError::Blank);
        assert_eq!(error.to_string(), "Тестове повідомлення: цей текст не можна залишити порожнім — ним перевіряється бот.");
    }

    #[test]
    fn battery_round_trips_through_its_form() {
        let battery = Battery::from_form("address=c8%3A47%3A8C%3Af2%3A7E%3A03&random=1&name=JK-B2A24S").unwrap();
        assert_eq!(battery.address, [0xC8, 0x47, 0x8C, 0xF2, 0x7E, 0x03]);
        assert!(battery.random);
        assert_eq!(battery.name, "JK-B2A24S");
        assert_eq!(battery.address_text(), "C8:47:8C:F2:7E:03");
        assert_eq!(Battery::from_form(&battery.to_form()), Ok(battery));
        assert!(!Battery::from_form("address=C8:47:8C:F2:7E:03").unwrap().random);
    }

    #[test]
    fn divider_reads_ohms_and_scales_the_pin_voltage() {
        let divider = Divider::from_form("top=330000&bottom=+10000+").unwrap();
        // 1.53 V on the pin is 52.02 V at the battery.
        assert_eq!(divider.battery_mv(1_530), 52_020);
        assert_eq!(Divider::from_form(&divider.to_form()), Ok(divider));
    }

    #[test]
    fn divider_needs_whole_ohms() {
        let bad = [("", "10000"), ("330000", "0"), ("330k", "10000"), ("-1", "10000"), ("330000", "1.5"), ("100000001", "10000")];
        for (top, bottom) in bad {
            let body = form::encode(&[("top", top), ("bottom", bottom)]);
            assert_eq!(Divider::from_form(&body), Err(SettingsError::Divider), "{top} / {bottom}");
        }
    }

    #[test]
    fn built_in_divider_needs_both_resistors_or_neither() {
        let divider = Divider::from_form("top=240000&bottom=10000").unwrap();
        assert_eq!(Divider::built_in(Some("240000"), Some("10000")), Ok(divider));
        assert!(Divider::built_in(Some("330000"), None).is_err());
        assert!(Divider::built_in(None, Some("10000")).is_err());
        assert!(Divider::built_in(Some("330k"), Some("10000")).is_err());
    }

    #[test]
    fn without_built_in_values_the_divider_is_the_one_on_sheet_1() {
        let sheet = Divider::from_form("top=330000&bottom=10000").unwrap();
        assert_eq!(Divider::built_in(None, None), Ok(sheet));
    }

    #[test]
    fn a_divider_never_saved_is_the_built_in_one() {
        let built_in = Divider::from_form("top=240000&bottom=10000").unwrap();
        assert_eq!(Divider::from_stored(None, built_in), Some(built_in));
    }

    #[test]
    fn a_divider_switched_off_stays_off_despite_the_built_in_one() {
        let built_in = Divider::SHEET_1;
        assert_eq!(Divider::from_stored(Some(&Divider::to_stored(None)), built_in), None);
    }

    #[test]
    fn a_saved_divider_beats_the_built_in_one() {
        let built_in = Divider::SHEET_1;
        let saved = Divider::from_form("top=100000&bottom=4700").ok();
        assert_eq!(Divider::from_stored(Some(&Divider::to_stored(saved)), built_in), saved);
    }

    #[test]
    fn battery_address_must_be_six_hex_bytes() {
        for address in ["", "C8:47:8C:F2:7E", "C8:47:8C:F2:7E:03:01", "C8:47:8C:F2:7E:G3", "C847:8C:F2:7E:03:"] {
            let body = form::encode(&[("address", address)]);
            assert_eq!(Battery::from_form(&body), Err(SettingsError::BmsAddress), "{address}");
        }
    }
}
