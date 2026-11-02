//! Mains power monitor: watches 220 V through an optocoupler on GPIO4 and
//! reports every change to Telegram. Without saved Wi-Fi (or when the saved
//! network is unreachable at start-up) the device opens a setup access point
//! with a captive portal that walks through Wi-Fi and bot setup; afterwards
//! the same page is a dashboard.

mod bms;
mod network;
mod notifier;
mod ota;
mod remote;
mod sensor;
mod settings;
mod status;
mod telegram;
mod web;

use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::peripherals::Peripherals;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::hal::reset::ResetReason;
use power_core::reset::{self, BootAction, Start, QUICK_BOOT_MS};

use crate::settings::Settings;
use crate::status::Status;

// The image header: version from build.rs, date and time of this build.
// ESP-IDF's own stamp is only refreshed when ESP-IDF itself is rebuilt.
// `true`: date and time each in their own field (the macro cannot tell
// `()` from `{}`, so only the explicit argument picks the fixed layout).
esp_idf_svc::sys::esp_app_desc!(true);

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::EspLogger::initialize_default();

    let peripherals = Peripherals::take()?;
    let sys_loop = EspSystemEventLoop::take()?;
    let nvs = EspDefaultNvsPartition::take()?;
    let settings = Arc::new(Settings::load(nvs.clone())?);
    count_quick_boot(&settings)?;
    let status = Arc::new(Status::new(ota::running()));
    status.set_telegram(if settings.bot().is_some() { "очікує подій" } else { "бот не налаштовано" });

    let bms = bms::Bms::start(settings.clone())?;
    let (notices, inbox) = mpsc::channel();
    sensor::spawn(peripherals.pins.gpio4, notices.clone(), status.clone())?;
    notifier::spawn(inbox, settings.clone(), status.clone(), bms.clone())?;
    let network = network::start(peripherals.modem, sys_loop, nvs, settings.clone(), status.clone(), notices)?;
    let remote = remote::Remote::start(settings.clone(), status.clone())?;
    let _server = web::start(settings, status, network, bms, remote)?;
    // Up far enough to take another update: keep this image.
    ota::confirm_running();

    // The work happens in other threads; this one keeps the server alive.
    loop {
        thread::park();
    }
}

/// Hardware random bytes; true random once the radio is running.
pub fn random<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    // SAFETY: writes exactly `bytes.len()` bytes into the buffer.
    unsafe { esp_idf_svc::sys::esp_fill_random(bytes.as_mut_ptr().cast(), bytes.len()) };
    bytes
}

/// Five power cycles in a row, each within 10 s of start-up, wipe the
/// settings: the way back in for someone who forgot the page password.
/// Only real power-ons count; brownouts and crashes leave the count alone.
fn count_quick_boot(settings: &Arc<Settings>) -> anyhow::Result<()> {
    let reason = ResetReason::get();
    let start = if reason == ResetReason::PowerOn { Start::PowerOn } else { Start::Other };
    match reset::on_boot(settings.quick_boots(), start) {
        BootAction::FactoryReset => {
            log::warn!("factory reset: {} quick start-ups in a row", reset::QUICK_BOOTS_FOR_RESET);
            settings.factory_reset()?;
            esp_idf_svc::hal::reset::restart();
        }
        BootAction::Count(count) => {
            log::info!("quick start-up {count} of {}", reset::QUICK_BOOTS_FOR_RESET);
            settings.set_quick_boots(count)?;
        }
        BootAction::Keep => log::warn!("started after {reason:?}, not a power-on: not counted"),
    }
    let settings = settings.clone();
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(QUICK_BOOT_MS));
        if let Err(error) = settings.set_quick_boots(0) {
            log::error!("clearing quick start-ups: {error}");
        }
    });
    Ok(())
}
