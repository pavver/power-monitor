//! User-editable message templates. `{device}`, `{battery_voltage}` and the
//! BMS values (`{bms_soc}`, … see `jk::VARIABLES`) fit every text; the
//! values in `OWN` only the texts that have them.
//!
//! `{name?text}` is a section: `text` (which may hold placeholders and
//! sections) shows only when `name` has a value, so a missing value can
//! take its whole sentence away instead of showing a shrug; `{!name?text}`
//! shows `text` only when `name` has none, so a second value can stand in
//! for the first. `\n` in the
//! text is a line break, for fields where a real one is awkward to type.

pub use autotext::template::{render, render_to, TemplateError, MAX_BYTES, MAX_CHARS, UNKNOWN};

use crate::jk;

/// The battery voltage measured through the divider.
pub const BATTERY_VOLTAGE: &str = "battery_voltage";

/// Values only some texts have: how long the previous state lasted and how
/// late the message is (power changes), the new address (its own message).
pub const OWN: [&str; 3] = ["duration", "delay", "ip"];

/// Checks a template for a text that has the `own` values of `OWN`.
pub fn validate(template: &str, own: &[&str]) -> Result<(), TemplateError> {
    autotext::template::validate(
        template,
        &|name| match name {
            "device" | BATTERY_VOLTAGE | crate::clock::TIME | crate::clock::DATE => None,
            name if jk::VARIABLES.contains(&name) || own.contains(&name) => None,
            name => match OWN.into_iter().find(|&other| other == name) {
                Some(other) => Some(TemplateError::NotHere(other)),
                None => Some(TemplateError::Unknown),
            },
        },
        &|_| false,
    )
}

/// Whether `template` shows any battery value.
pub fn uses_bms(template: &str) -> bool {
    autotext::template::uses(template, &|name| jk::VARIABLES.contains(&name))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a power change offers on top of the common values.
    const CHANGE: &[&str] = &["duration", "delay"];

    #[test]
    fn each_text_offers_its_own_values() {
        assert_eq!(validate("{delay?(сталося {delay} тому)}", CHANGE), Ok(()));
        assert_eq!(validate("http://{ip}/", &["ip"]), Ok(()));
        assert_eq!(validate("{ip}", CHANGE), Err(TemplateError::NotHere("ip")));
        assert_eq!(validate("{!delay?вчасно}", &["ip"]), Err(TemplateError::NotHere("delay")));
        assert_eq!(validate("{device} {battery_voltage} {bms_soc}", &[]), Ok(()));
    }

    #[test]
    fn says_which_value_is_not_here() {
        assert_eq!(TemplateError::NotHere("ip").to_string(), "змінна {ip} у цьому тексті недоступна");
    }

    #[test]
    fn checks_names_of_negated_sections() {
        assert_eq!(validate("{!bms_soc?{battery_voltage} В}", &[]), Ok(()));
        assert_eq!(validate("{!duration?a}", &[]), Err(TemplateError::NotHere("duration")));
        assert_eq!(validate("{!foo?a}", CHANGE), Err(TemplateError::Unknown));
        assert_eq!(validate("{!device?a", CHANGE), Err(TemplateError::Unclosed));
        assert!(uses_bms("{!bms_soc?нема BMS}"));
    }

    #[test]
    fn checks_names_inside_and_of_sections() {
        assert_eq!(validate("{duration?Не було {duration}.}", CHANGE), Ok(()));
        assert_eq!(validate("{bms_soc?{bms_soc}%}", &[]), Ok(()));
        assert_eq!(validate("{duration?a}", &[]), Err(TemplateError::NotHere("duration")));
        assert_eq!(validate("{device?{foo}}", CHANGE), Err(TemplateError::Unknown));
        assert_eq!(validate("{foo?a}", CHANGE), Err(TemplateError::Unknown));
    }

    #[test]
    fn a_battery_value_in_a_section_needs_the_battery() {
        assert!(uses_bms("{bms_soc?Заряд {bms_soc}%}"));
        assert!(uses_bms("{device?{bms_soc}}"));
    }

    #[test]
    fn accepts_battery_values_in_every_message() {
        assert_eq!(validate("{bms_soc}% {bms_voltage} В {bms_energy}", &[]), Ok(()));
        assert_eq!(validate("{battery_voltage} В", &[]), Ok(()));
        assert_eq!(validate("{bms_nonsense}", &[]), Err(TemplateError::Unknown));
        assert_eq!(validate("{bms_temp1}", &[]), Err(TemplateError::Unknown));
    }

    #[test]
    fn knows_when_the_battery_is_needed() {
        assert!(uses_bms("Заряд {bms_soc}%"));
        assert!(!uses_bms("{device}: {duration}"));
        assert!(!uses_bms("bms_soc без дужок"));
        assert!(uses_bms("Енергія {bms_energy} кВт·год"));
    }

    #[test]
    fn accepts_known_placeholders() {
        assert_eq!(validate("⚡ {device}: світло є о {time} {date}", &[]), Ok(()));
        assert_eq!(validate("Не було {duration} на {device}", CHANGE), Ok(()));
        assert_eq!(validate("{ просто дужки }", &[]), Ok(()));
    }

    #[test]
    fn rejects_duration_where_it_does_not_exist() {
        assert_eq!(validate("{duration}", &[]), Err(TemplateError::NotHere("duration")));
    }

    #[test]
    fn rejects_unknown_placeholders() {
        assert_eq!(validate("{foo}", CHANGE), Err(TemplateError::Unknown));
        assert_eq!(validate("{Device}", CHANGE), Err(TemplateError::Unknown));
    }
}
