//! Error types for templates, automations, and live status policies.

use std::fmt;

use autotext::automation::AutomationError;
use autotext::library::CallError;
use autotext::template::TemplateError;

/// Validation and parsing errors for texts, automations, and status policies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextsError {
    Template(TemplateError),
    Call(CallError),
    Automation(AutomationError),
    BadTemplateName(String),
    MissingField(&'static str),
    UnknownTrigger(String),
    UnknownAction(String),
    InvalidNumber(String),
    InvalidTime(String),
    StatusEvery,
    UnknownSensor(String),
    InvalidLiveId,
    InvalidLiveBottom,
    TooManyTemplates,
    TemplateNotFound(String),
    TemplateInUse { name: String, callers: Vec<String> },
}

impl fmt::Display for TextsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Template(e) => write!(f, "{e}"),
            Self::Call(e) => write!(f, "{e}"),
            Self::Automation(e) => write!(f, "{e}"),
            Self::BadTemplateName(name) => {
                write!(
                    f,
                    "неприпустима назва шаблону '{name}': латинські малі літери, цифри та _, до 15 символів"
                )
            }
            Self::MissingField(field) => write!(f, "відсутнє обов'язкове поле: {field}"),
            Self::UnknownTrigger(trig) => write!(f, "невідомий тригер: {trig}"),
            Self::UnknownAction(act) => write!(f, "невідома дія: {act}"),
            Self::InvalidNumber(s) => write!(f, "некоректне число: {s}"),
            Self::InvalidTime(s) => write!(f, "некоректний час '{s}', очікується ГГ:ХХ (наприклад 08:30)"),
            Self::StatusEvery => write!(f, "інтервал оновлення статусу має бути від 10 до 86400 секунд"),
            Self::UnknownSensor(s) => write!(f, "невідомий сенсор: {s}"),
            Self::InvalidLiveId => write!(f, "некоректний ідентифікатор повідомлення"),
            Self::InvalidLiveBottom => write!(f, "некоректне значення bottom (очікується 0 або 1)"),
            Self::TooManyTemplates => {
                write!(f, "досягнуто ліміт кількості шаблонів (максимум 32)")
            }
            Self::TemplateNotFound(name) => write!(f, "шаблон '{name}' не знайдено"),
            Self::TemplateInUse { name, callers } => {
                write!(
                    f,
                    "неможливо видалити шаблон '{name}', його використовують: {}",
                    callers.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for TextsError {}
