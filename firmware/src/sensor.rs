//! Samples the optocoupler once per millisecond and feeds the detector.

use std::sync::mpsc::Sender;
use std::sync::Arc;

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{Input, InputPin, PinDriver, Pull};
use power_core::message::Notice;
use power_core::power::{Detector, PowerState};

use crate::status::{now_ms, Status};

/// Samples per window, one per FreeRTOS tick (1 ms, see sdkconfig.defaults).
const WINDOW_SAMPLES: u32 = 100;
/// Ten 100 ms windows: a state must hold for a second to count.
const CONFIRM_WINDOWS: u32 = 10;

pub fn spawn(
    pin: impl InputPin + 'static,
    notices: Sender<Notice>,
    status: Arc<Status>,
    kept: Option<PowerState>,
) -> anyhow::Result<()> {
    let input = PinDriver::input(pin, Pull::Up)?;
    // After a planned restart only a change is news; the start-up is not.
    let mut detector = match kept {
        Some(state) => {
            status.set_power(state, None);
            Detector::resume(CONFIRM_WINDOWS, state)
        }
        None => Detector::new(CONFIRM_WINDOWS),
    };
    crate::tasks::spawn("sensor", 4096, move || {
        loop {
            let started_ms = now_ms();
            if let Some(event) = detector.observe(window(&input), started_ms) {
                log::info!("power {:?}", event.state);
                status.set_power(event.state, event.began_ms());
                // Nobody listens until the bot is configured; that is fine.
                let _ = notices.send(Notice::Power(event));
            }
        }
    })?;
    Ok(())
}

#[cfg(not(mock_mains))]
fn window(input: &PinDriver<'_, Input>) -> PowerState {
    power_core::power::classify_window(count_low_samples(input))
}

/// What a mock build's page set, held for one window's time.
#[cfg(mock_mains)]
fn window(_input: &PinDriver<'_, Input>) -> PowerState {
    FreeRtos::delay_ms(WINDOW_SAMPLES);
    if mock::PRESENT.load(std::sync::atomic::Ordering::Relaxed) {
        PowerState::Present
    } else {
        PowerState::Absent
    }
}

#[cfg(mock_mains)]
pub mod mock {
    use std::sync::atomic::AtomicBool;

    /// The mains state a mock build pretends to see.
    pub static PRESENT: AtomicBool = AtomicBool::new(true);
}

#[cfg_attr(mock_mains, allow(dead_code))]
fn count_low_samples(input: &PinDriver<'_, Input>) -> u32 {
    (0..WINDOW_SAMPLES)
        .filter(|_| {
            let low = input.is_low();
            FreeRtos::delay_ms(1);
            low
        })
        .count() as u32
}
