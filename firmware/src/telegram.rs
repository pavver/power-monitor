//! HTTPS calls to the Telegram Bot API.

use std::time::Duration;

use embedded_svc::http::client::Client;
use embedded_svc::io::Write;
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use power_core::settings::{Bot, BotToken};
use power_core::telegram::{self, Chat};
use serde_json::Value;

/// `getUpdates` can carry many messages; larger answers are cut and rejected.
const MAX_RESPONSE: usize = 48 * 1024;

pub fn send(bot: &Bot, text: &str) -> Result<(), String> {
    send_to(bot.token(), bot.chat_id(), text)
}

pub fn send_to(token: &BotToken, chat_id: i64, text: &str) -> Result<(), String> {
    let request = telegram::send_message(token, chat_id, text);
    call(&request.url, Some(&request.body)).map(|_| ())
}

/// The bot's `@username`, proving the token works.
pub fn bot_username(token: &BotToken) -> Result<String, String> {
    let me = call(&telegram::get_me(token), None)?;
    telegram::bot_username(&me).ok_or_else(|| "Telegram не повернув ім'я бота".to_string())
}

pub fn chats(token: &BotToken) -> Result<Vec<Chat>, String> {
    Ok(telegram::chats(&call(&telegram::get_updates(token), None)?))
}

/// POSTs `body` (or GETs without one) and returns the call's `result`.
fn call(url: &str, body: Option<&str>) -> Result<Value, String> {
    let (status, response) = exchange(url, body).map_err(|e| format!("немає зв'язку з Telegram ({e})"))?;
    telegram::result(status, &response)
}

fn exchange(url: &str, body: Option<&str>) -> anyhow::Result<(u16, String)> {
    let connection = EspHttpConnection::new(&Configuration {
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        timeout: Some(Duration::from_secs(20)),
        ..Default::default()
    })?;
    let mut client = Client::wrap(connection);
    let length = body.map_or(0, str::len).to_string();
    let headers = [
        ("content-type", "application/x-www-form-urlencoded"),
        ("content-length", length.as_str()),
    ];
    let mut response = match body {
        None => client.get(url)?.submit()?,
        Some(body) => {
            let mut post = client.post(url, &headers)?;
            post.write_all(body.as_bytes())?;
            post.flush()?;
            post.submit()?
        }
    };
    let status = response.status();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let read = response.read(&mut chunk)?;
        if read == 0 || bytes.len() + read > MAX_RESPONSE {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    Ok((status, String::from_utf8_lossy(&bytes).into_owned()))
}
