//! The JSON API, in groups by what the page sets up.

use std::sync::mpsc::Sender;
use std::sync::Arc;

use embedded_svc::http::{Headers, Method};
use esp_idf_svc::http::server::EspHttpServer;
use power_core::api::Link;
use power_core::auth::{expired_cookie, NewPassword, PasswordHash};
use power_core::form;
use power_core::remote::RemoteSettings;
use power_core::settings::{Battery, Bot, Divider, WifiCredentials};

use super::{
    accept, deny, get, json, json_text, login, planned_restart, post, post_text, read_body, respond, restart_soon,
    token,
    Answer, Auth, MAX_FORM,
};
use crate::bms::Bms;
use crate::network::Command;
use crate::notifier::BMS_MAX_AGE;
use crate::ota;
use crate::random;
use crate::remote::Remote;
use crate::settings::Settings;
use crate::status::Status;
use crate::telegram;
use crate::voltmeter::Voltmeter;

type Server = EspHttpServer<'static>;

/// What the handlers work with.
pub(super) struct Device {
    pub settings: Arc<Settings>,
    pub status: Arc<Status>,
    pub network: Sender<Command>,
    pub bms: Bms,
    pub voltmeter: Voltmeter,
    pub remote: Remote,
}

impl Device {
    /// Sends the test message to `bot`, with the battery and status as it is now.
    fn send_test(&self, bot: &Bot) -> Result<(), String> {
        let status_tpl = self.settings.template(power_core::texts::STATUS_TEMPLATE).unwrap_or_default();
        let device_name = self.settings.with_messages(|m| m.device_name().to_string());
        let last_ip = self.settings.last_ip();
        let bms_reading = if self.settings.battery().is_some() {
            self.bms.recent_reading(BMS_MAX_AGE)
        } else {
            None
        };
        let snap = power_core::texts::Snapshot {
            device: &device_name,
            mains: self.status.power(),
            ip: last_ip.as_deref(),
            voltage_mv: self.voltmeter.battery_mv(),
            bms: bms_reading.as_ref(),
            local: crate::clock::local(),
        };
        let v = power_core::texts::values(&snap);
        let mut status_rendered = String::new();
        let _ = autotext::template::render_with(&status_tpl, &v.as_slice(), &*self.settings, &mut status_rendered);
        let text = format!("🔔 Тестове повідомлення\n\n{status_rendered}");
        telegram::send(bot, &text).map(|_| ())
    }
}

/// Wi-Fi and the state the page shows.
pub(super) fn setup(server: &mut Server, auth: &Arc<Auth>, d: &Arc<Device>) -> anyhow::Result<()> {
    let dev = d.clone();
    // Asked every 2 s while the panel is open: serialized once, not read
    // back into a tree to add to it.
    get(server, auth, "/api/state", move || {
        let body = dev.status.state_json(&dev.settings, &dev.remote.view());
        Ok(Answer { body, cookie: None })
    })?;

    let dev = d.clone();
    get(server, auth, "/api/networks", move || {
        let _ = dev.network.send(Command::Scan);
        json(&dev.status.networks())
    })?;

    let dev = d.clone();
    post(server, auth, "/api/setup/close", move |_| {
        dev.network.send(Command::CloseSetup).map_err(|e| e.to_string())?;
        json(&"closing")
    })?;

    let dev = d.clone();
    post(server, auth, "/api/wifi", move |body| {
        let credentials = WifiCredentials::from_form(body).map_err(|e| e.to_string())?;
        // Set before replying, so the page never mistakes the previous
        // attempt's result for this one's.
        dev.status.set_link(Link::Connecting { ssid: credentials.ssid().to_string() });
        dev.network.send(Command::Connect(credentials)).map_err(|e| e.to_string())?;
        json(&"connecting")
    })
}

/// The bot and the message texts.
pub(super) fn bot(server: &mut Server, auth: &Arc<Auth>, d: &Arc<Device>) -> anyhow::Result<()> {
    post(server, auth, "/api/bot/check", |body| {
        let username = telegram::bot_username(&token(body)?)?;
        json(&serde_json::json!({ "username": username }))
    })?;

    post(server, auth, "/api/bot/chats", |body| json(&telegram::chats(&token(body)?)?))?;

    let dev = d.clone();
    post(server, auth, "/api/bot", move |body| {
        let bot = Bot::from_form(body).map_err(|e| e.to_string())?;
        dev.send_test(&bot)?;
        dev.settings.save_bot(bot).map_err(|e| e.to_string())?;
        json(&"saved")
    })?;

    let dev = d.clone();
    // `?field=<key>`, and the text as it is.
    post_text(server, auth, "/api/messages", move |query, text| {
        dev.settings.save_message_field(query, text).map_err(|e| e.to_string())?.map_err(|e| e.to_string())?;
        json(&"saved")
    })?;

    let dev = d.clone();
    post(server, auth, "/api/test", move |_| {
        let bot = dev.settings.bot().ok_or("Спершу налаштуйте бота.")?;
        dev.send_test(&bot)?;
        json(&"sent")
    })
}

fn automation_view(a: &autotext::automation::Automation) -> serde_json::Value {
    use autotext::automation::{Action, Trigger};
    let form = power_core::texts::automation_to_form(a);
    let mut obj = serde_json::Map::new();
    obj.insert("form".to_string(), serde_json::Value::String(form));
    match &a.trigger {
        Trigger::StartUp => {
            obj.insert("on".to_string(), serde_json::Value::String("start".to_string()));
        }
        Trigger::Change { sensor } => {
            obj.insert("on".to_string(), serde_json::Value::String("change".to_string()));
            obj.insert("sensor".to_string(), serde_json::Value::String(sensor.clone()));
        }
        Trigger::Becomes { sensor, value } => {
            obj.insert("on".to_string(), serde_json::Value::String("becomes".to_string()));
            obj.insert("sensor".to_string(), serde_json::Value::String(sensor.clone()));
            obj.insert("value".to_string(), serde_json::Value::String(value.clone()));
        }
        Trigger::Step { sensor, step } => {
            obj.insert("on".to_string(), serde_json::Value::String("step".to_string()));
            obj.insert("sensor".to_string(), serde_json::Value::String(sensor.clone()));
            obj.insert("step".to_string(), serde_json::json!(step));
        }
        Trigger::Below { sensor, threshold, hysteresis } => {
            obj.insert("on".to_string(), serde_json::Value::String("below".to_string()));
            obj.insert("sensor".to_string(), serde_json::Value::String(sensor.clone()));
            obj.insert("threshold".to_string(), serde_json::json!(threshold));
            obj.insert("hysteresis".to_string(), serde_json::json!(hysteresis));
        }
        Trigger::Above { sensor, threshold, hysteresis } => {
            obj.insert("on".to_string(), serde_json::Value::String("above".to_string()));
            obj.insert("sensor".to_string(), serde_json::Value::String(sensor.clone()));
            obj.insert("threshold".to_string(), serde_json::json!(threshold));
            obj.insert("hysteresis".to_string(), serde_json::json!(hysteresis));
        }
        Trigger::At { minute } => {
            obj.insert("on".to_string(), serde_json::Value::String("at".to_string()));
            obj.insert("at".to_string(), serde_json::Value::String(format!("{:02}:{:02}", minute / 60, minute % 60)));
        }
        Trigger::Every { seconds } => {
            obj.insert("on".to_string(), serde_json::Value::String("every".to_string()));
            obj.insert("every".to_string(), serde_json::json!(seconds));
        }
    }
    obj.insert("if".to_string(), serde_json::json!(a.conditions));
    let do_str = match a.action {
        Action::Send => "send",
        Action::UpdateStatus => "status",
    };
    obj.insert("do".to_string(), serde_json::Value::String(do_str.to_string()));
    obj.insert("text".to_string(), serde_json::Value::String(a.text.clone()));
    serde_json::Value::Object(obj)
}

/// Templates, automations, and live status policy.
pub(super) fn texts(server: &mut Server, auth: &Arc<Auth>, d: &Arc<Device>) -> anyhow::Result<()> {
    let dev = d.clone();
    // What every value is now, for the page's pickers (missing ones are null).
    get(server, auth, "/api/system", || json(&crate::system::view()))?;
    get(server, auth, "/api/values", move || {
        let values = current_values(&dev);
        let map: serde_json::Map<String, serde_json::Value> = values
            .as_slice()
            .iter()
            .map(|(name, value)| (name.to_string(), value.map_or(serde_json::Value::Null, |v| v.into())))
            .collect();
        json(&map)
    })?;

    let dev = d.clone();
    get(server, auth, "/api/texts", move || {
        let tmpl_names = dev.settings.templates();
        let mut templates = Vec::with_capacity(tmpl_names.len());
        for name in &tmpl_names {
            let text = dev.settings.template(name).unwrap_or_default();
            templates.push(serde_json::json!({ "name": name, "text": text }));
        }

        let autos = dev.settings.automations();
        let automations: Vec<serde_json::Value> = autos.iter().map(automation_view).collect();

        let cfg = dev.settings.status_config();
        let status = serde_json::json!({
            "every": cfg.every_ms / 1000,
            "important": cfg.important,
            "bottom": cfg.keep_at_bottom,
        });

        json(&serde_json::json!({
            "values": power_core::texts::NAMES,
            "templates": templates,
            "automations": automations,
            "status": status,
            "max_templates": power_core::texts::MAX_TEMPLATES,
            "max_automations": autotext::automation::MAX_AUTOMATIONS,
        }))
    })?;

    let dev = d.clone();
    accept(server, auth, "/api/template", autotext::template::MAX_BYTES, Box::new(move |query, body| {
        let pairs = form::parse(query);
        let name = form::field(&pairs, "name");
        if name.is_empty() {
            return Err("Вкажіть назву шаблону.".to_string());
        }
        dev.settings.save_template(name, body)
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        json(&"saved")
    }))?;

    let dev = d.clone();
    accept(server, auth, "/api/template/delete", autotext::template::MAX_BYTES, Box::new(move |query, _| {
        let pairs = form::parse(query);
        let name = form::field(&pairs, "name");
        if name.is_empty() {
            return Err("Вкажіть назву шаблону.".to_string());
        }
        dev.settings.delete_template(name)
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        json(&"deleted")
    }))?;

    let dev = d.clone();
    accept(server, auth, "/api/template/preview", autotext::template::MAX_BYTES, Box::new(move |_, body| {
        autotext::template::validate(
            body,
            &|var| if power_core::texts::known(var) { None } else { Some(autotext::template::TemplateError::Unknown) },
            &|tmpl| dev.settings.has_template(tmpl),
        ).map_err(|e| power_core::texts::TextsError::Template(e).to_string())?;

        autotext::library::check_calls("", body, &*dev.settings)
            .map_err(|e| power_core::texts::TextsError::Call(e).to_string())?;

        let v = current_values(&dev);
        let mut out = String::new();
        autotext::template::render_with(body, &v.as_slice(), &*dev.settings, &mut out)
            .map_err(|e| e.to_string())?;
        json(&serde_json::json!({ "text": out }))
    }))?;

    let dev = d.clone();
    accept(server, auth, "/api/automation", MAX_FORM, Box::new(move |query, body| {
        let pairs = form::parse(query);
        let index_str = form::field(&pairs, "index");
        let index = if index_str.is_empty() {
            None
        } else {
            Some(index_str.parse::<usize>().map_err(|_| "Некоректний індекс.".to_string())?)
        };
        dev.settings.save_automation(index, body)
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        json(&"saved")
    }))?;

    let dev = d.clone();
    accept(server, auth, "/api/automation/delete", MAX_FORM, Box::new(move |query, _| {
        let pairs = form::parse(query);
        let index_str = form::field(&pairs, "index");
        let index = index_str.parse::<usize>().map_err(|_| "Некоректний індекс.".to_string())?;
        dev.settings.delete_automation(index)
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        json(&"deleted")
    }))?;

    let dev = d.clone();
    post(server, auth, "/api/status", move |body| {
        dev.settings.save_status_config(body)
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        json(&"saved")
    })?;

    Ok(())
}

/// The BMS and the voltage divider.
pub(super) fn battery(server: &mut Server, auth: &Arc<Auth>, d: &Arc<Device>) -> anyhow::Result<()> {
    // Asking for the values is what keeps the BMS link up: the battery
    // screen polls this while it is open.
    let dev = d.clone();
    get(server, auth, "/api/bms", move || json(&dev.bms.view()))?;

    let dev = d.clone();
    post(server, auth, "/api/bms/scan", move |_| {
        if dev.bms.scan() {
            return json(&"scanning");
        }
        // Bluetooth was not started at start-up: start again with it.
        dev.settings.ask_bluetooth_once().map_err(|e| e.to_string())?;
        planned_restart(&dev.settings, &dev.status);
        json(&"restarting")
    })?;

    let dev = d.clone();
    post(server, auth, "/api/bms", move |body| {
        let battery = Battery::from_form(body).map_err(|e| e.to_string())?;
        dev.settings.save_battery(Some(battery)).map_err(|e| e.to_string())?;
        dev.bms.forget();
        json(&"saved")
    })?;

    let dev = d.clone();
    post(server, auth, "/api/bms/forget", move |_| {
        dev.settings.save_battery(None).map_err(|e| e.to_string())?;
        dev.bms.forget();
        json(&"forgotten")
    })?;

    let dev = d.clone();
    get(server, auth, "/api/divider", move || {
        // `noisy`: the pin wanders, as it does with no divider fitted.
        let pin = dev.voltmeter.pin();
        let noisy = pin.is_some_and(|pin| pin.steady_mv().is_none());
        let spread_mv = pin.map(|pin| pin.spread_mv);
        let voltage_mv = dev.voltmeter.battery_mv();
        json(&serde_json::json!({ "divider": dev.settings.divider(), "voltage_mv": voltage_mv, "noisy": noisy, "spread_mv": spread_mv }))
    })?;

    // Both fields empty: no divider fitted.
    let dev = d.clone();
    post(server, auth, "/api/divider", move |body| {
        let fields = form::parse(body);
        let off = ["top", "bottom"].iter().all(|name| form::field(&fields, name).trim().is_empty());
        let divider = if off { None } else { Some(Divider::from_form(body).map_err(|e| e.to_string())?) };
        dev.settings.save_divider(divider).map_err(|e| e.to_string())?;
        json(&"saved")
    })
}

/// Remote support.
pub(super) fn remote(server: &mut Server, auth: &Arc<Auth>, d: &Arc<Device>) -> anyhow::Result<()> {
    let dev = d.clone();
    post(server, auth, "/api/remote", move |body| {
        let settings = RemoteSettings::from_form(body).map_err(|e| e.to_string())?;
        dev.settings.save_remote(settings).map_err(|e| e.to_string())?;
        dev.remote.reconfigure();
        json(&"saved")
    })?;

    let dev = d.clone();
    post(server, auth, "/api/remote/connect", move |_| {
        dev.remote.connect_now();
        json(&"connecting")
    })?;

    let dev = d.clone();
    post(server, auth, "/api/remote/end", move |_| {
        dev.remote.end();
        json(&"ended")
    })
}

/// Login, the page password, the firmware and the reset.
pub(super) fn access(server: &mut Server, auth: &Arc<Auth>, d: &Arc<Device>) -> anyhow::Result<()> {
    let a = auth.clone();
    server.fn_handler::<anyhow::Error, _>("/api/login", Method::Post, move |mut req| {
        let reply = read_body(&mut req, MAX_FORM).and_then(|body| login(&a, &body));
        respond(req, reply)
    })?;

    let a = auth.clone();
    server.fn_handler::<anyhow::Error, _>("/api/logout", Method::Post, move |req| {
        a.sessions.lock().unwrap().close(req.header("Cookie"));
        respond(req, Ok(Answer { body: json_text(&"bye"), cookie: Some(expired_cookie()) }))
    })?;

    let a = auth.clone();
    post(server, auth, "/api/password", move |body| {
        let password = NewPassword::from_form(body).map_err(|e| e.to_string())?;
        a.settings.save_password(Some(PasswordHash::new(&password, random()))).map_err(|e| e.to_string())?;
        // Everyone else has to log in with the new password; this browser stays in.
        a.sessions.lock().unwrap().close_all();
        Ok(Answer { body: json_text(&"saved"), cookie: Some(a.open_session()) })
    })?;

    let a = auth.clone();
    post(server, auth, "/api/password/off", move |_| {
        a.settings.save_password(None).map_err(|e| e.to_string())?;
        a.sessions.lock().unwrap().close_all();
        json(&"off")
    })?;

    let dev = d.clone();
    post(server, auth, "/api/factory-reset", move |_| {
        dev.settings.factory_reset().map_err(|e| e.to_string())?;
        restart_soon();
        json(&"reset")
    })?;

    let (a, dev) = (auth.clone(), d.clone());
    server.fn_handler::<anyhow::Error, _>("/api/ota", Method::Post, move |mut req| {
        if !a.allows(req.header("Cookie")) {
            return deny(req);
        }
        let size = req.content_len().unwrap_or(0) as usize;
        let reply = ota::update(&mut req, size).and_then(|info| {
            planned_restart(&dev.settings, &dev.status);
            json(&serde_json::json!({ "version": info.version, "built": info.built }))
        });
        respond(req, reply)
    })?;
    Ok(())
}

/// Time zone and wall-clock time.
pub(super) fn clock(server: &mut Server, auth: &Arc<Auth>, d: &Arc<Device>) -> anyhow::Result<()> {
    let dev = d.clone();
    get(server, auth, "/api/clock", move || {
        let tz = dev.settings.time_zone();
        let local = crate::clock::local();
        let time = local.as_ref().map(power_core::clock::Local::time);
        let date = local.as_ref().map(power_core::clock::Local::date);
        json(&serde_json::json!({
            "tz": tz.as_str(),
            "time": time,
            "date": date,
        }))
    })?;

    let dev = d.clone();
    post(server, auth, "/api/clock", move |body| {
        let fields;
        let text = if body.starts_with("tz=") {
            fields = form::parse(body);
            form::field(&fields, "tz")
        } else {
            body
        };
        let tz = power_core::clock::TimeZone::parse(text).map_err(|e| e.to_string())?;
        dev.settings.save_time_zone(tz.clone()).map_err(|e| e.to_string())?;
        crate::clock::apply(&tz);
        json(&"saved")
    })
}

/// The values templates see right now, as the notifier sees them.
fn current_values(dev: &Device) -> power_core::texts::Values {
    let device_name = dev.settings.with_messages(|m| m.device_name().to_string());
    let last_ip = dev.settings.last_ip();
    let bms_reading = if dev.settings.battery().is_some() {
        dev.bms.recent_reading(BMS_MAX_AGE)
    } else {
        None
    };
    let snap = power_core::texts::Snapshot {
        device: &device_name,
        mains: dev.status.power(),
        ip: last_ip.as_deref(),
        voltage_mv: dev.voltmeter.battery_mv(),
        bms: bms_reading.as_ref(),
        local: crate::clock::local(),
    };
    power_core::texts::values(&snap)
}
