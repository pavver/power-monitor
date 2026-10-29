//! HTTPS call to the Telegram Bot API.

use std::time::Duration;

use embedded_svc::http::client::Client;
use embedded_svc::io::Write;
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use power_core::config::Config;

pub fn send(config: &Config, text: &str) -> anyhow::Result<()> {
    let request = power_core::telegram::send_message(config.bot_token(), config.chat_id(), text);
    let connection = EspHttpConnection::new(&Configuration {
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        timeout: Some(Duration::from_secs(20)),
        ..Default::default()
    })?;
    let mut client = Client::wrap(connection);
    let length = request.body.len().to_string();
    let headers = [
        ("content-type", "application/x-www-form-urlencoded"),
        ("content-length", length.as_str()),
    ];
    let mut post = client.post(&request.url, &headers)?;
    post.write_all(request.body.as_bytes())?;
    post.flush()?;
    match post.submit()?.status() {
        200 => Ok(()),
        401 | 404 => anyhow::bail!("Telegram не приймає токен бота"),
        400 | 403 => anyhow::bail!("Telegram не знайшов чат — напишіть боту /start і перевірте Chat ID"),
        code => anyhow::bail!("Telegram відповів кодом {code}"),
    }
}
