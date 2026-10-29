//! What the web page shows. Each field has exactly one writer thread:
//! power — sensor, network — Wi-Fi supervisor, telegram and pending — notifier.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use power_core::html;
use power_core::power::PowerState;

/// Milliseconds since boot.
pub fn now_ms() -> u64 {
    // SAFETY: esp_timer is started by ESP-IDF before `main`.
    let micros = unsafe { esp_idf_svc::sys::esp_timer_get_time() };
    micros as u64 / 1_000
}

#[derive(Default)]
pub struct Status {
    power: Mutex<Option<(PowerState, u64)>>,
    network: Mutex<String>,
    telegram: Mutex<String>,
    pending: AtomicUsize,
}

impl Status {
    pub fn set_power(&self, state: PowerState, since_ms: u64) {
        *self.power.lock().unwrap() = Some((state, since_ms));
    }

    pub fn set_network(&self, text: impl Into<String>) {
        *self.network.lock().unwrap() = text.into();
    }

    pub fn set_telegram(&self, text: impl Into<String>) {
        *self.telegram.lock().unwrap() = text.into();
    }

    pub fn set_pending(&self, count: usize) {
        self.pending.store(count, Ordering::Relaxed);
    }

    /// Renders the page with the current values.
    pub fn page(&self, config: Option<&power_core::config::Config>, notice: Option<&str>) -> String {
        let power = self
            .power
            .lock()
            .unwrap()
            .map(|(state, since)| (state, now_ms().saturating_sub(since)));
        let network = self.network.lock().unwrap().clone();
        let telegram = self.telegram.lock().unwrap().clone();
        let status = html::Status {
            power,
            network: &network,
            telegram: &telegram,
            pending: self.pending.load(Ordering::Relaxed),
        };
        html::page(&status, config, notice)
    }
}
