use super::*;
use autotext::automation::AutomationError;
use autotext::library::{CallError, NoTemplates};

#[test]
fn check_template_valid() {
    assert!(check_template("svitlo", "{mains=on?Є}", &NoTemplates).is_ok());
}

#[test]
fn check_template_bad_name() {
    assert_eq!(
        check_template("Svitlo", "{mains}", &NoTemplates),
        Err(TextsError::BadTemplateName("Svitlo".to_string()))
    );
    assert_eq!(
        check_template("", "{mains}", &NoTemplates),
        Err(TextsError::BadTemplateName("".to_string()))
    );
    assert_eq!(
        check_template("way_too_long_template_name", "{mains}", &NoTemplates),
        Err(TextsError::BadTemplateName("way_too_long_template_name".to_string()))
    );
}

#[test]
fn check_template_unknown_variable() {
    assert_eq!(
        check_template("test", "{nonexistent}", &NoTemplates),
        Err(TextsError::Template(TemplateError::Unknown))
    );
}

#[test]
fn check_template_call_cycle() {
    let lib = [("a", "{@b}"), ("b", "{@a}")];
    let err = check_template("a", "{@b}", &lib.as_slice());
    assert!(matches!(err, Err(TextsError::Call(CallError::Cycle(_)))));
}

#[test]
fn automation_form_round_trip() {
    let auto = Automation {
        trigger: Trigger::Step {
            sensor: "bms_soc".to_string(),
            step: 10.0,
        },
        conditions: vec!["mains=off".to_string(), "bms_soc<50".to_string()],
        action: Action::Send,
        text: "{device}: заряд {bms_soc}%".to_string(),
    };

    let encoded = automation_to_form(&auto);
    let decoded = automation_from_form(&encoded, &NoTemplates, &|_| false).unwrap();
    assert_eq!(decoded, auto);
}

#[test]
fn automation_form_at_trigger() {
    let auto = Automation {
        trigger: Trigger::At { minute: 8 * 60 + 30 },
        conditions: Vec::new(),
        action: Action::UpdateStatus,
        text: "оновлення".to_string(),
    };

    let encoded = automation_to_form(&auto);
    let decoded = automation_from_form(&encoded, &NoTemplates, &|_| false).unwrap();
    assert_eq!(decoded, auto);
}

#[test]
fn automation_from_form_rejects_bad_input() {
    let err = automation_from_form("on=unknown&do=send&text=hi", &NoTemplates, &|_| false);
    assert!(matches!(err, Err(TextsError::UnknownTrigger(_))));

    let err = automation_from_form(
        "on=step&sensor=unknown&step=5&do=send&text=hi",
        &NoTemplates,
        &|_| false,
    );
    assert!(matches!(
        err,
        Err(TextsError::Automation(AutomationError::UnknownSensor(_)))
    ));
}

#[test]
fn status_config_form_round_trip() {
    let config = StatusConfig {
        every_ms: 120_000,
        important: vec!["mains".to_string(), "bms_soc".to_string()],
        keep_at_bottom: true,
    };

    let encoded = status_config_to_form(&config);
    assert_eq!(encoded, "every=120&important=mains%2Cbms_soc&bottom=1");
    let decoded = status_config_from_form(&encoded).unwrap();
    assert_eq!(decoded, config);
}

#[test]
fn status_config_from_form_rejects_out_of_range_every() {
    assert_eq!(
        status_config_from_form("every=5&important=mains&bottom=1"),
        Err(TextsError::StatusEvery)
    );
    assert_eq!(
        status_config_from_form("every=90000&important=mains&bottom=1"),
        Err(TextsError::StatusEvery)
    );
}

#[test]
fn live_form_round_trip_distinguishing_none_and_empty() {
    let live = Live {
        message_id: 123456789,
        snapshot: vec![
            Some("on".to_string()),
            None,
            Some("".to_string()),
            Some("192.168.1.1".to_string()),
        ],
        updated_ms: 50_000,
        at_bottom: true,
    };

    let encoded = live_to_form(&live);
    let decoded = live_from_form(&encoded).unwrap();
    assert_eq!(decoded.message_id, 123456789);
    assert_eq!(decoded.snapshot, live.snapshot);
    assert_eq!(decoded.updated_ms, 0);
    assert_eq!(decoded.at_bottom, true);
}
