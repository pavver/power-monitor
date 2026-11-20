//! The device name and the message texts, with how often to remind that
//! the power is still off.

use serde::Serialize;

use super::SettingsError;
use crate::form;
use crate::jk;
use crate::template::{self, TemplateError};

/// The editable texts, one per kind of message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text {
    StartPresent,
    StartAbsent,
    Lost,
    Returned,
    Address,
    Test,
    StillOff,
}

impl Text {
    pub const ALL: [Text; 7] =
        [Text::StartPresent, Text::StartAbsent, Text::Lost, Text::Returned, Text::StillOff, Text::Address, Text::Test];

    /// Form field, JSON key and settings key (at most 15 bytes).
    pub fn key(self) -> &'static str {
        match self {
            Text::StartPresent => "start_present",
            Text::StartAbsent => "start_absent",
            Text::Lost => "lost",
            Text::Returned => "returned",
            Text::Address => "address",
            Text::Test => "test",
            Text::StillOff => "still_off",
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Text::StartPresent => "Запуск, світло є",
            Text::StartAbsent => "Запуск, світла немає",
            Text::Lost => "Світло зникло",
            Text::Returned => "Світло з'явилось",
            Text::Address => "Нова адреса панелі",
            Text::Test => "Тестове повідомлення",
            Text::StillOff => "Світла все ще немає",
        }
    }

    /// The values of `template::OWN` this text has. Start-up messages have
    /// no previous state to measure and are never late.
    pub(crate) fn own(self) -> &'static [&'static str] {
        match self {
            Text::Lost | Text::Returned | Text::StillOff => &["duration", "delay"],
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
            Text::Lost => concat!("🔌 {device}: світло зникло.{duration?\nБуло {duration}.}{delay?\n(сталося {delay} тому)}", battery!()),
            Text::Returned => concat!("⚡ {device}: світло з'явилось.{duration?\nНе було {duration}.}{delay?\n(сталося {delay} тому)}", battery!()),
            Text::Address => "🌐 {device}: нова адреса панелі http://{ip}/",
            Text::Test => concat!("🔔 {device}: тестове повідомлення, бот працює.", battery!()),
            Text::StillOff => concat!("🔌 {device}: світла все ще немає.{duration?\nНемає вже {duration}.}{delay?\n(станом на {delay} тому)}", battery!()),
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

/// What the page needs to edit one text: its standard wording and the
/// values it may use, in the order they are offered.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub struct About {
    pub standard: &'static str,
    pub variables: Vec<&'static str>,
}

/// The text named by `query` (`field=<Text::key>`), or `None` for anything else.
pub fn about(query: &str) -> Option<About> {
    let fields = form::parse(query);
    let text = Text::ALL.into_iter().find(|text| text.key() == form::field(&fields, "field"))?;
    let mut variables = vec!["device"];
    variables.extend_from_slice(text.own());
    variables.push(template::BATTERY_VOLTAGE);
    variables.extend(jk::VARIABLES);
    Some(About { standard: text.default_template(), variables })
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
    still_off: String,
    /// Minutes between reminders that the power is still off.
    still_off_every: u16,
}

const DEFAULT_DEVICE_NAME: &str = "Дім";
const DEFAULT_STILL_OFF_EVERY: u16 = 60;
const STILL_OFF_EVERY: &str = "still_off_every";

/// Minutes between reminders: 1 to a day.
fn still_off_every(value: &str) -> Option<u16> {
    value.trim().parse().ok().filter(|minutes| (1..=1440).contains(minutes))
}

impl Default for Messages {
    fn default() -> Self {
        let default = |text: Text| Ok::<_, ()>(text.default_template().to_string());
        Self::build(DEFAULT_DEVICE_NAME.to_string(), DEFAULT_STILL_OFF_EVERY, default).unwrap()
    }
}

impl Messages {
    /// Changes the field named by `query` (`field=<key>`, the key being
    /// "device_name", "still_off_every" or a `Text::key`) to `value`, the
    /// text as it came; returns the key and the value as it is to be stored
    /// (same as `fields()` would give).
    pub fn set(&mut self, query: &str, value: &str) -> Result<(&'static str, String), SettingsError> {
        let fields = form::parse(query);
        let field = form::field(&fields, "field");
        if field == "device_name" {
            let name = device_name(value).ok_or(SettingsError::DeviceName)?;
            self.device_name = name.clone();
            return Ok(("device_name", name));
        }
        if field == STILL_OFF_EVERY {
            let every = still_off_every(value).ok_or(SettingsError::StillOffEvery)?;
            self.still_off_every = every;
            return Ok((STILL_OFF_EVERY, every.to_string()));
        }
        if let Some(text) = Text::ALL.into_iter().find(|t| t.key() == field) {
            let text_val = text.parse(value).map_err(|e| SettingsError::Text(text, e))?;
            *self.text_mut(text) = text_val.clone();
            return Ok((text.key(), text_val));
        }
        Err(SettingsError::UnknownField)
    }

    fn text_mut(&mut self, text: Text) -> &mut String {
        match text {
            Text::StartPresent => &mut self.start_present,
            Text::StartAbsent => &mut self.start_absent,
            Text::Lost => &mut self.lost,
            Text::Returned => &mut self.returned,
            Text::Address => &mut self.address,
            Text::Test => &mut self.test,
            Text::StillOff => &mut self.still_off,
        }
    }

    /// Reads `device_name` and one field per `Text::key`, as the page sends them.
    pub fn from_form(body: &str) -> Result<Self, SettingsError> {
        let fields = form::parse(body);
        let device_name = device_name(form::field(&fields, "device_name")).ok_or(SettingsError::DeviceName)?;
        let every = still_off_every(form::field(&fields, STILL_OFF_EVERY)).ok_or(SettingsError::StillOffEvery)?;
        Self::build(device_name, every, |text| text.parse(form::field(&fields, text.key())).map_err(|error| SettingsError::Text(text, error)))
    }

    /// What `fields` stored, `get` giving each by its key; a missing or
    /// broken one, e.g. a text from a newer firmware, is the default.
    pub fn from_stored(get: impl Fn(&str) -> Option<String>) -> Self {
        let device_name = get("device_name").as_deref().and_then(device_name).unwrap_or(DEFAULT_DEVICE_NAME.to_string());
        let every = get(STILL_OFF_EVERY).as_deref().and_then(still_off_every).unwrap_or(DEFAULT_STILL_OFF_EVERY);
        let text = |text: Text| get(text.key()).and_then(|value| text.parse(&value).ok());
        Self::build(device_name, every, |t| Ok::<_, ()>(text(t).unwrap_or(t.default_template().to_string()))).unwrap()
    }

    /// Every value with its key, for storing.
    pub fn fields(&self) -> Vec<(&'static str, String)> {
        let mut fields = vec![("device_name", self.device_name.clone()), (STILL_OFF_EVERY, self.still_off_every.to_string())];
        fields.extend(Text::ALL.map(|text| (text.key(), self.template(text).to_string())));
        fields
    }

    pub fn still_off_every_ms(&self) -> u64 {
        u64::from(self.still_off_every) * 60_000
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
            Text::StillOff => &self.still_off,
        }
    }

    fn build<E>(device_name: String, still_off_every: u16, mut text: impl FnMut(Text) -> Result<String, E>) -> Result<Self, E> {
        Ok(Self {
            device_name,
            still_off_every,
            start_present: text(Text::StartPresent)?,
            start_absent: text(Text::StartAbsent)?,
            lost: text(Text::Lost)?,
            returned: text(Text::Returned)?,
            address: text(Text::Address)?,
            test: text(Text::Test)?,
            still_off: text(Text::StillOff)?,
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

    #[test]
    fn a_text_is_described_by_its_standard_wording_and_its_values() {
        let lost = about("field=lost").unwrap();
        assert_eq!(lost.standard, Messages::default().template(Text::Lost));
        assert_eq!(
            lost.variables,
            [
                "device",
                "duration",
                "delay",
                "battery_voltage",
                "bms_soc",
                "bms_voltage",
                "bms_energy",
                "bms_current",
                "bms_power"
            ]
        );
        assert_eq!(about("field=address").unwrap().variables[..2], ["device", "ip"]);
        assert_eq!(about("field=test").unwrap().variables[..2], ["device", "battery_voltage"]);
        assert_eq!(about("field=device_name"), None);
    }

    fn messages_form(overrides: &[(&str, &str)]) -> String {
        let mut fields = Messages::default().fields();
        for (key, value) in overrides {
            fields.iter_mut().find(|(k, _)| k == key).unwrap().1 = value.to_string();
        }
        encode(&fields)
    }

    fn encode(fields: &[(&str, String)]) -> String {
        form::encode(&fields.iter().map(|(k, v)| (*k, v.as_str())).collect::<Vec<_>>())
    }

    #[test]
    fn messages_read_every_text() {
        let m = Messages::from_form(&messages_form(&[("device_name", " Дача "), ("lost", "Нема {duration}")]))
            .unwrap();
        assert_eq!(m.device_name(), "Дача");
        assert_eq!(m.template(Text::Lost), "Нема {duration}");
        assert_eq!(m.template(Text::Returned), Text::Returned.default_template());
        assert_eq!(Messages::from_form(&encode(&m.fields())), Ok(m));
    }

    #[test]
    fn messages_read_how_often_to_remind() {
        let m = Messages::from_form(&messages_form(&[("still_off_every", " 30 ")])).unwrap();
        assert_eq!(m.still_off_every_ms(), 30 * 60_000);
        assert_eq!(Messages::default().still_off_every_ms(), 60 * 60_000);
        for every in ["", "0", "1441", "x", "-5"] {
            let body = messages_form(&[("still_off_every", every)]);
            assert_eq!(Messages::from_form(&body), Err(SettingsError::StillOffEvery), "{every}");
        }
    }

    #[test]
    fn stored_reminder_interval_round_trips_or_is_the_default() {
        let m = Messages::from_form(&messages_form(&[("still_off_every", "15")])).unwrap();
        let stored = m.fields();
        let back = Messages::from_stored(|key| stored.iter().find(|(k, _)| *k == key).map(|(_, v)| v.to_string()));
        assert_eq!(back.still_off_every_ms(), 15 * 60_000);
        let broken = Messages::from_stored(|key| (key == "still_off_every").then(|| "0".to_string()));
        assert_eq!(broken, Messages::default());
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
    fn a_blank_text_is_kept_as_empty() {
        let m = Messages::from_form(&messages_form(&[("returned", " \n "), ("start_absent", "")])).unwrap();
        assert_eq!(m.template(Text::Returned), "");
        assert_eq!(m.template(Text::StartAbsent), "");
        assert_eq!(Messages::from_form(&encode(&m.fields())), Ok(m));
    }

    #[test]
    fn default_templates_are_valid() {
        assert_eq!(Messages::from_form(&messages_form(&[])), Ok(Messages::default()));
    }

    #[test]
    fn errors_explain_which_text_is_wrong() {
        let error = SettingsError::Text(Text::Lost, TemplateError::TooLong);
        assert_eq!(error.to_string(), "Світло зникло: текст довший за 256 символів.");
        let error = SettingsError::Text(Text::Lost, TemplateError::TooBig);
        assert_eq!(error.to_string(), "Світло зникло: текст займає понад 512 байтів (емодзі — по 4 байти).");
        let error = SettingsError::Text(Text::Test, TemplateError::Blank);
        assert_eq!(error.to_string(), "Тестове повідомлення: цей текст не можна залишити порожнім — ним перевіряється бот.");
    }

    #[test]
    fn every_text_is_named_as_the_page_names_it() {
        let labels = Text::ALL.map(Text::label);
        assert_eq!(
            labels,
            [
                "Запуск, світло є",
                "Запуск, світла немає",
                "Світло зникло",
                "Світло з'явилось",
                "Світла все ще немає",
                "Нова адреса панелі",
                "Тестове повідомлення",
            ]
        );
    }

    #[test]
    fn setting_one_text_changes_only_it() {
        let mut m = Messages::default();
        let initial = m.clone();
        let res = m.set("field=lost", "Світло зникло! {duration}").unwrap();
        assert_eq!(res, ("lost", "Світло зникло! {duration}".to_string()));
        assert_eq!(m.template(Text::Lost), "Світло зникло! {duration}");
        assert_eq!(m.device_name(), initial.device_name());
        assert_eq!(m.still_off_every_ms(), initial.still_off_every_ms());
        for text in Text::ALL {
            if text != Text::Lost {
                assert_eq!(m.template(text), initial.template(text));
            }
        }
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
    fn the_text_is_taken_as_it_came() {
        let mut m = Messages::default();
        m.set("field=lost", "50%+ & {duration}\n🔋").unwrap();
        assert_eq!(m.template(Text::Lost), "50%+ & {duration}\n🔋");
    }

    #[test]
    fn setting_still_off_every() {
        let mut m = Messages::default();
        let res = m.set("field=still_off_every", "15").unwrap();
        assert_eq!(res, ("still_off_every", "15".to_string()));
        assert_eq!(m.still_off_every_ms(), 15 * 60_000);

        let before = m.clone();
        for bad in ["0", "1441", "x", ""] {
            assert_eq!(m.set("field=still_off_every", bad), Err(SettingsError::StillOffEvery), "{bad}");
            assert_eq!(m, before);
        }
    }

    #[test]
    fn setting_blank_text_stored_as_empty_and_blank_test_text_errors() {
        let mut m = Messages::default();
        let res = m.set("field=address", " \n ").unwrap();
        assert_eq!(res, ("address", "".to_string()));
        assert_eq!(m.template(Text::Address), "");

        let before = m.clone();
        assert_eq!(m.set("field=test", " "), Err(SettingsError::Text(Text::Test, TemplateError::Blank)));
        assert_eq!(m, before);
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
