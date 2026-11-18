//! The JSON API, in groups by what the page sets up.

use std::sync::mpsc::Sender;
use std::sync::Arc;

use embedded_svc::http::{Headers, Method};
use esp_idf_svc::http::server::EspHttpServer;
use power_core::api::Link;
use power_core::auth::{expired_cookie, NewPassword, PasswordHash};
use power_core::form;
use power_core::message::{preview, test_text, Readings};
use power_core::remote::RemoteSettings;
use power_core::settings::{Battery, Bot, Divider, WifiCredentials};

use super::{
    deny, get, json, json_text, login, planned_restart, post, post_text, read_body, respond, restart_soon, system_view,
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
    /// Sends the test message to `bot`, with the battery as it is now.
    fn send_test(&self, bot: &Bot) -> Result<(), String> {
        let reading = self.bms.recent_reading(BMS_MAX_AGE);
        let readings = Readings { bms: reading.as_ref(), voltage_mv: self.voltmeter.battery_mv() };
        telegram::send(bot, &self.settings.with_messages(|messages| test_text(messages, readings)))
    }
}

/// Wi-Fi and the state the page shows.
pub(super) fn setup(server: &mut Server, auth: &Arc<Auth>, d: &Arc<Device>) -> anyhow::Result<()> {
    let dev = d.clone();
    // Asked every 2 s while the panel is open: serialized once, not read
    // back into a tree to add to it.
    get(server, auth, "/api/state", move || {
        let body = dev.status.state_json(&dev.settings, &dev.remote.view(), &system_view());
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

    // `?field=<key>`: the standard wording and the values one text may use.
    post_text(server, auth, "/api/messages/about", |query, _| {
        json(&power_core::settings::about(query).ok_or("Невідомий текст.")?)
    })?;

    // The page asks while a text is edited, so the device's own rules fill it.
    let dev = d.clone();
    post_text(server, auth, "/api/messages/preview", move |query, template| {
        let reading = dev.bms.last_reading();
        json(&preview(query, template, Readings { bms: reading.as_ref(), voltage_mv: dev.voltmeter.battery_mv() })?)
    })?;

    let dev = d.clone();
    post(server, auth, "/api/test", move |_| {
        let bot = dev.settings.bot().ok_or("Спершу налаштуйте бота.")?;
        dev.send_test(&bot)?;
        json(&"sent")
    })
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
