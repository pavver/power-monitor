//! HTTP server: the status/settings page, saving settings, a test message,
//! and a catch-all redirect so captive-portal checks land on the page.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use embedded_svc::http::{Headers, Method};
use embedded_svc::io::{Read, Write};
use esp_idf_svc::http::server::{Configuration, EspHttpConnection, EspHttpServer, Request};
use power_core::config::Config;
use power_core::message::test_text;

use crate::status::Status;
use crate::storage::Storage;
use crate::telegram;

const MAX_BODY: usize = 2048;

pub fn start(
    storage: Storage,
    config: Option<Config>,
    status: Arc<Status>,
) -> anyhow::Result<EspHttpServer<'static>> {
    let mut server = EspHttpServer::new(&Configuration {
        uri_match_wildcard: true,
        // The test message runs a TLS handshake inside the handler.
        stack_size: 12 * 1024,
        ..Default::default()
    })?;
    let storage = Arc::new(Mutex::new(storage));
    let config = Arc::new(config);

    let (s, c) = (status.clone(), config.clone());
    server.fn_handler::<anyhow::Error, _>("/", Method::Get, move |req| {
        respond(req, 200, &s.page(c.as_ref().as_ref(), None))
    })?;

    let (s, c) = (status.clone(), config.clone());
    server.fn_handler::<anyhow::Error, _>("/save", Method::Post, move |mut req| {
        let body = read_body(&mut req)?;
        match Config::from_form(&body, c.as_ref().as_ref()) {
            Ok(new_config) => {
                storage.lock().unwrap().save(&new_config)?;
                restart_soon();
                let notice = "Збережено. Пристрій перезавантажується…";
                respond(req, 200, &s.page(Some(&new_config), Some(notice)))
            }
            Err(error) => respond(req, 400, &s.page(c.as_ref().as_ref(), Some(&error.to_string()))),
        }
    })?;

    let (s, c) = (status.clone(), config.clone());
    server.fn_handler::<anyhow::Error, _>("/test", Method::Post, move |req| {
        let notice = match c.as_ref() {
            None => "Спершу збережіть налаштування.".to_string(),
            Some(config) => match telegram::send(config, &test_text(config.device_name())) {
                Ok(()) => "Тестове повідомлення надіслано.".to_string(),
                Err(error) => format!("Не вдалося надіслати: {error}"),
            },
        };
        respond(req, 200, &s.page(c.as_ref().as_ref(), Some(&notice)))
    })?;

    server.fn_handler::<anyhow::Error, _>("/*", Method::Get, |req| {
        req.into_response(302, None, &[("Location", "/")])?;
        Ok(())
    })?;

    Ok(server)
}

fn respond(req: Request<&mut EspHttpConnection>, code: u16, html: &str) -> anyhow::Result<()> {
    let mut response = req.into_response(code, None, &[("Content-Type", "text/html; charset=utf-8")])?;
    response.write_all(html.as_bytes())?;
    Ok(())
}

fn read_body(req: &mut Request<&mut EspHttpConnection>) -> anyhow::Result<String> {
    let len = req.content_len().unwrap_or(0) as usize;
    anyhow::ensure!(len <= MAX_BODY, "request body too large");
    let mut body = vec![0; len];
    req.read_exact(&mut body)?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// Lets the response reach the browser before the reboot.
fn restart_soon() {
    thread::spawn(|| {
        thread::sleep(Duration::from_secs(2));
        esp_idf_svc::hal::reset::restart();
    });
}
