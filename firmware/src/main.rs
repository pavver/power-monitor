//! Mains power monitor: watches 220 V through an optocoupler on GPIO4 and
//! reports every change to Telegram. Without saved Wi-Fi (or when the saved
//! network is unreachable at start-up) the device opens a setup access point
//! with a captive portal that walks through Wi-Fi and bot setup; afterwards
//! the same page is a dashboard.

mod network;
mod notifier;
mod sensor;
mod settings;
mod status;
mod telegram;
mod web;

use std::sync::mpsc;
use std::sync::Arc;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use crate::settings::Settings;
use crate::status::Status;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    let settings = Arc::new(Settings::load(nvs.clone())?);
    let status = Arc::new(Status::default());
    status.set_telegram(if settings.bot().is_some() { "очікує подій" } else { "бот не налаштовано" });

    let (events_tx, events_rx) = mpsc::channel();
    sensor::spawn(peripherals.pins.gpio4, events_tx, status.clone())?;
    notifier::spawn(events_rx, settings.clone(), status.clone())?;
    let network = network::start(peripherals.modem, sys_loop, nvs, settings.clone(), status.clone())?;
    let _server = web::start(settings, status, network)?;

    // The work happens in other threads; this one keeps the server alive.
    loop {
        std::thread::park();
    }
}
