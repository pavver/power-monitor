//! Live values shown on the page. Each field has exactly one writer thread:
//! power — sensor; link, setup_ap, networks — network; telegram, pending — notifier.
//! The firmware description is fixed at start-up.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use power_core::api::{self, Firmware, Link, Network, Power};
use power_core::power::PowerState;

use crate::settings::Settings;

/// Milliseconds since boot.
pub fn now_ms() -> u64 {
    // SAFETY: esp_timer is started by ESP-IDF before `main`.
    let micros = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
    micros as u64 / 1_000
}

#[derive(Default)]
pub struct Status {
    power: Mutex<Option<(PowerState, u64)>>,
    link: Mutex<Link>,
    setup_ap: Mutex<Option<&'static str>>,
    networks: Mutex<Vec<Network>>,
    telegram: Mutex<String>,
    pending: AtomicUsize,
    firmware: Firmware,
}

impl Status {
    pub fn new(firmware: Firmware) -> Self {
        Self { firmware, ..Default::default() }
    }

    pub fn set_power(&self, state: PowerState, since_ms: u64) {
        *self.power.lock().unwrap() = Some((state, since_ms));
    }

    pub fn set_link(&self, link: Link) {
        *self.link.lock().unwrap() = link;
    }

    pub fn link(&self) -> Link {
        self.link.lock().unwrap().clone()
    }

    pub fn set_setup_ap(&self, ssid: &'static str) {
        *self.setup_ap.lock().unwrap() = Some(ssid);
    }

    pub fn set_networks(&self, networks: Vec<Network>) {
        *self.networks.lock().unwrap() = networks;
    }

    pub fn networks(&self) -> Vec<Network> {
        self.networks.lock().unwrap().clone()
    }

    pub fn set_telegram(&self, text: impl Into<String>) {
        *self.telegram.lock().unwrap() = text.into();
    }

    pub fn set_pending(&self, count: usize) {
        self.pending.store(count, Ordering::Relaxed);
    }

    /// The page state as JSON.
    pub fn state_json(&self, settings: &Settings) -> String {
        let bot = settings.bot();
        let messages = settings.messages();
        let link = self.link();
        let telegram = self.telegram.lock().unwrap().clone();
        let power = self
            .power
            .lock()
            .unwrap()
            .map(|(state, since)| Power::new(state, now_ms().saturating_sub(since)));
        let state = api::State {
            stage: api::stage(settings.wifi().is_some(), bot.is_some()),
            power,
            link: &link,
            setup_ap: *self.setup_ap.lock().unwrap(),
            telegram: &telegram,
            pending: self.pending.load(Ordering::Relaxed),
            chat_id: bot.map(|b| b.chat_id()),
            messages: &messages,
            firmware: &self.firmware,
            password_set: settings.password().is_some(),
        };
        serde_json::to_string(&state).unwrap_or_default()
    }
}
