//! Default templates, automations, and status configuration.

use autotext::automation::{Action, Automation, Trigger};
use autotext::status::StatusConfig;

/// Default templates provided out of the box.
pub const DEFAULT_TEMPLATES: [(&str, &str); 6] = [
    (
        "svitlo",
        "{mains=on?Мережа увімкнена ✅}{mains=off?Мережа вимкнена ❌}{!mains?⚠️ Датчик мережі недоступний}",
    ),
    (
        "battery_icon",
        "{!bms_soc?❔}{bms_soc>=80?🔋}{bms_soc<80?{bms_soc>=50?🔋½}{bms_soc<50?{bms_soc>=20?🪫}{bms_soc<20?🪫‼️}}}",
    ),
    (
        "battery",
        "{@battery_icon} {bms_soc?{bms_soc}% ({bms_energy} кВт·год)}{!bms_soc?{battery_voltage?{battery_voltage} В}{!battery_voltage?⚠️ Дані батареї недоступні}}",
    ),
    (
        "load",
        "{bms_current>0?⚡ Заряджається: {bms_current} А ({bms_power} Вт)}{bms_current<0?🔻 Розряджається: {bms_current} А ({bms_power} Вт)}{bms_current=0?💤 Без струму}",
    ),
    (
        "update_time",
        "{time?🕒 Оновлено о {time}}",
    ),
    (
        "status",
        "{device}: {@svitlo}\n{@battery}{bms_current?\n{@load}}{time?\n\n{@update_time}}",
    ),
];

/// Returns the standard default templates.
pub fn default_templates() -> [(&'static str, &'static str); 6] {
    DEFAULT_TEMPLATES
}

/// Returns the standard automations installed by default.
pub fn default_automations() -> Vec<Automation> {
    vec![Automation {
        trigger: Trigger::Change {
            sensor: "ip".to_string(),
        },
        conditions: Vec::new(),
        action: Action::Send,
        text: "🌐 {device}: нова адреса панелі http://{ip}/".to_string(),
    }]
}

/// Returns the default status message configuration.
pub fn default_status_config() -> StatusConfig {
    StatusConfig {
        every_ms: 300_000,
        important: vec!["mains".to_string()],
        keep_at_bottom: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock;
    use crate::jk;
    use crate::power::PowerState;
    use crate::texts::values::{self, values, Snapshot};
    use autotext::template::render_with;

    #[test]
    fn default_templates_pass_validation_and_check_calls() {
        let lib = DEFAULT_TEMPLATES;
        let has_tmpl = |name: &str| lib.iter().any(|(n, _)| *n == name);

        for (name, text) in DEFAULT_TEMPLATES {
            autotext::template::validate(
                text,
                &|v| if values::known(v) { None } else { Some(autotext::template::TemplateError::Unknown) },
                &has_tmpl,
            )
            .unwrap_or_else(|e| panic!("template '{name}' failed validation: {e}"));

            autotext::library::check_calls(name, text, &lib)
                .unwrap_or_else(|e| panic!("template '{name}' failed check_calls: {e}"));
        }
    }

    #[test]
    fn default_automation_passes_validation() {
        let autos = default_automations();
        assert_eq!(autos.len(), 1);
        let lib = DEFAULT_TEMPLATES;
        let has_tmpl = |name: &str| lib.iter().any(|(n, _)| *n == name);

        for auto in &autos {
            autotext::automation::validate(auto, &values::known)
                .unwrap_or_else(|e| panic!("default automation failed validation: {e}"));

            autotext::template::validate(
                &auto.text,
                &|v| if values::known(v) { None } else { Some(autotext::template::TemplateError::Unknown) },
                &has_tmpl,
            )
            .unwrap_or_else(|e| panic!("default automation text failed template validation: {e}"));

            autotext::library::check_calls("", &auto.text, &lib)
                .unwrap_or_else(|e| panic!("default automation text failed check_calls: {e}"));
        }
    }

    fn render_status(snap: &Snapshot) -> String {
        let v = values(snap);
        let slice = v.as_slice();
        let lib = DEFAULT_TEMPLATES;
        let mut out = String::new();
        let status_tmpl = lib.iter().find(|(k, _)| *k == "status").unwrap().1;
        render_with(status_tmpl, &slice, &lib, &mut out).unwrap();
        out
    }

    #[test]
    fn render_default_status_case1_bms_charging() {
        let bms = jk::Reading {
            voltage_mv: 53_200,
            soc: 92,
            remaining_mah: 120_000,
            current_ma: 12_500,
            power_mw: 665_000,
        };
        let snap = Snapshot {
            device: "Дім",
            mains: Some(PowerState::Present),
            ip: Some("192.168.1.50"),
            voltage_mv: Some(53_200),
            bms: Some(&bms),
            local: Some(clock::Local::new(2026, 11, 20, 14, 30)),
        };

        let rendered = render_status(&snap);
        let expected = "Дім: Мережа увімкнена ✅\n\
                        🔋 92% (6.38 кВт·год)\n\
                        ⚡ Заряджається: 12.5 А (665 Вт)\n\n\
                        🕒 Оновлено о 14:30";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn render_default_status_case2_bms_discharging() {
        let bms = jk::Reading {
            voltage_mv: 50_800,
            soc: 45,
            remaining_mah: 60_000,
            current_ma: -6_200,
            power_mw: -315_000,
        };
        let snap = Snapshot {
            device: "Дім",
            mains: Some(PowerState::Absent),
            ip: Some("192.168.1.50"),
            voltage_mv: Some(50_800),
            bms: Some(&bms),
            local: Some(clock::Local::new(2026, 11, 20, 18, 0)),
        };

        let rendered = render_status(&snap);
        let expected = "Дім: Мережа вимкнена ❌\n\
                        🪫 45% (3.05 кВт·год)\n\
                        🔻 Розряджається: -6.2 А (-315 Вт)\n\n\
                        🕒 Оновлено о 18:00";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn render_default_status_case3_divider_only() {
        let snap = Snapshot {
            device: "Дім",
            mains: Some(PowerState::Present),
            ip: Some("192.168.1.50"),
            voltage_mv: Some(12_450),
            bms: None,
            local: Some(clock::Local::new(2026, 11, 20, 10, 15)),
        };

        let rendered = render_status(&snap);
        let expected = "Дім: Мережа увімкнена ✅\n\
                        ❔ 12.45 В\n\n\
                        🕒 Оновлено о 10:15";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn render_default_status_case4_nothing_measured_and_clock_unset() {
        let snap = Snapshot {
            device: "Дім",
            mains: None,
            ip: None,
            voltage_mv: None,
            bms: None,
            local: None,
        };

        let rendered = render_status(&snap);
        let expected = "Дім: ⚠️ Датчик мережі недоступний\n\
                        ❔ ⚠️ Дані батареї недоступні";
        assert_eq!(rendered, expected);
    }
}
