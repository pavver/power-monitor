//! HTTP server: the single-page UI, its JSON API, and a catch-all redirect so
//! captive-portal checks land on the page.

use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use embedded_svc::http::{Headers, Method};
use embedded_svc::io::{Read, Write};
use esp_idf_svc::http::server::{Configuration, EspHttpConnection, EspHttpServer, Request};
use power_core::form;
use power_core::message::test_text;
use power_core::settings::{Bot, BotToken, Messages, WifiCredentials};
use serde::Serialize;

use crate::network::Command;
use crate::ota;
use crate::settings::Settings;
use crate::status::Status;
use crate::telegram;

const PAGE: &str = include_str!("../ui/index.html");
const MAX_BODY: usize = 4096;

type Req<'a, 'b> = Request<&'a mut EspHttpConnection<'b>>;
/// A JSON reply, or a message for the user shown with status 400.
type Reply = Result<String, String>;

pub fn start(
    settings: Arc<Settings>,
    status: Arc<Status>,
    network: Sender<Command>,
) -> anyhow::Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&Configuration {
        uri_match_wildcard: true,
        // Bot setup runs TLS handshakes inside handlers.
        stack_size: 14 * 1024,
        max_uri_handlers: 16,
        ..Default::default()
    })?;

    server.fn_handler::<anyhow::Error, _>("/", Method::Get, |req| {
        req.into_response(200, None, &[("Content-Type", "text/html; charset=utf-8")])?
            .write_all(PAGE.as_bytes())?;
        Ok(())
    })?;

    let (s, st) = (settings.clone(), status.clone());
    get(&mut server, "/api/state", move || Ok(st.state_json(&s)))?;

    let (st, net) = (status.clone(), network.clone());
    get(&mut server, "/api/networks", move || {
        let _ = net.send(Command::Scan);
        json(&st.networks())
    })?;

    post(&mut server, "/api/wifi", move |body| {
        let credentials = WifiCredentials::from_form(body).map_err(|e| e.to_string())?;
        network.send(Command::Connect(credentials)).map_err(|e| e.to_string())?;
        json(&"connecting")
    })?;

    post(&mut server, "/api/bot/check", |body| {
        let username = telegram::bot_username(&token(body)?)?;
        json(&serde_json::json!({ "username": username }))
    })?;

    post(&mut server, "/api/bot/chats", |body| json(&telegram::chats(&token(body)?)?))?;

    let s = settings.clone();
    post(&mut server, "/api/bot", move |body| {
        let bot = Bot::from_form(body).map_err(|e| e.to_string())?;
        telegram::send(&bot, &test_text(s.messages().device_name()))?;
        s.save_bot(bot).map_err(|e| e.to_string())?;
        json(&"saved")
    })?;

    let s = settings.clone();
    post(&mut server, "/api/messages", move |body| {
        let messages = Messages::from_form(body).map_err(|e| e.to_string())?;
        s.save_messages(messages).map_err(|e| e.to_string())?;
        json(&"saved")
    })?;

    post(&mut server, "/api/test", move |_| {
        let bot = settings.bot().ok_or("Спершу налаштуйте бота.")?;
        telegram::send(&bot, &test_text(settings.messages().device_name()))?;
        json(&"sent")
    })?;

    server.fn_handler::<anyhow::Error, _>("/api/ota", Method::Post, |mut req| {
        let size = req.content_len().unwrap_or(0) as usize;
        let reply = ota::update(&mut req, size).and_then(|info| {
            restart_soon();
            json(&serde_json::json!({ "version": info.version, "built": info.built }))
        });
        respond(req, reply)
    })?;

    server.fn_handler::<anyhow::Error, _>("/*", Method::Get, |req| {
        req.into_response(302, None, &[("Location", "/")])?;
        Ok(())
    })?;

    Ok(server)
}

fn token(body: &str) -> Result<BotToken, String> {
    BotToken::parse(form::field(&form::parse(body), "token")).map_err(|e| e.to_string())
}

fn json<T: Serialize + ?Sized>(value: &T) -> Reply {
    serde_json::to_string(value).map_err(|e| e.to_string())
}

fn get<F>(server: &mut EspHttpServer<'static>, uri: &str, handler: F) -> anyhow::Result<()>
where
    F: Fn() -> Reply + Send + Sync + 'static,
{
    server.fn_handler::<anyhow::Error, _>(uri, Method::Get, move |req| respond(req, handler()))?;
    Ok(())
}

fn post<F>(server: &mut EspHttpServer<'static>, uri: &str, handler: F) -> anyhow::Result<()>
where
    F: Fn(&str) -> Reply + Send + Sync + 'static,
{
    server.fn_handler::<anyhow::Error, _>(uri, Method::Post, move |mut req| {
        let reply = read_body(&mut req).and_then(|body| handler(&body));
        respond(req, reply)
    })?;
    Ok(())
}

fn respond(req: Req, reply: Reply) -> anyhow::Result<()> {
    let (code, body) = match reply {
        Ok(body) => (200, body),
        Err(message) => (400, serde_json::json!({ "error": message }).to_string()),
    };
    let headers = [("Content-Type", "application/json"), ("Cache-Control", "no-store")];
    req.into_response(code, None, &headers)?.write_all(body.as_bytes())?;
    Ok(())
}

fn read_body(req: &mut Req) -> Result<String, String> {
    let len = req.content_len().unwrap_or(0) as usize;
    if len > MAX_BODY {
        return Err("Запит завеликий.".to_string());
    }
    let mut body = vec![0; len];
    req.read_exact(&mut body).map_err(|e| format!("{e:?}"))?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Lets the response reach the browser before the reboot into the new image.
fn restart_soon() {
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(2));
        esp_idf_svc::hal::reset::restart();
    });
}
