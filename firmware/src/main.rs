//! Mains power monitor: watches 220 V through an optocoupler on GPIO4 and
//! reports every change to Telegram. Settings live in a web page; without
//! them (or when BOOT is pressed within 3 s of start-up) the device opens a
//! captive portal.

mod network;
mod notifier;
mod sensor;
mod status;
mod storage;
mod telegram;
mod web;

use std::sync::mpsc;
use std::sync::Arc;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{InputPin, PinDriver, Pull};
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;

use crate::status::Status;
use crate::storage::Storage;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    let storage = Storage::new(nvs.clone())?;

    let force_setup = boot_pressed_during_startup(peripherals.pins.gpio0)?;
    let config = storage.load().filter(|_| !force_setup);
    log::info!("config present: {}, forced setup: {force_setup}", config.is_some());

    let status = Arc::new(Status::default());
    status.set_telegram(if config.is_some() { "очікує подій" } else { "не налаштовано" });

    let (events_tx, events_rx) = mpsc::channel();
    sensor::spawn(peripherals.pins.gpio4, events_tx, status.clone())?;
    let wifi = network::start(peripherals.modem, sys_loop, nvs, config.as_ref(), &status)?;
    let _server = web::start(storage, config.clone(), status.clone())?;
    if let Some(config) = &config {
        notifier::spawn(config.clone(), events_rx, status.clone())?;
    }
    network::supervise(wifi, config.as_ref(), &status)
}

/// BOOT held at power-up selects the ROM bootloader, so the button is read
/// during the first seconds after start instead.
fn boot_pressed_during_startup(pin: impl InputPin) -> anyhow::Result<bool> {
    const WINDOW_MS: u32 = 3_000;
    log::info!("press BOOT within 3 s to open the setup access point");
    let button = PinDriver::input(pin, Pull::Up)?;
    Ok((0..WINDOW_MS / 50).any(|_| {
        FreeRtos::delay_ms(50);
        button.is_low()
    }))
}
