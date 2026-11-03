//! Samples the optocoupler once per millisecond and feeds the detector.

use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::thread;

use esp_idf_svc::hal::delay::FreeRtos;
use esp_idf_svc::hal::gpio::{Input, InputPin, PinDriver, Pull};
use power_core::message::Notice;
use power_core::power::{classify_window, Detector, PowerState};

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
    thread::Builder::new().stack_size(4096).spawn(move || {
        loop {
            let started_ms = now_ms();
            let low_samples = count_low_samples(&input);
            if let Some(event) = detector.observe(classify_window(low_samples), started_ms) {
                log::info!("power {:?}", event.state);
                status.set_power(event.state, event.began_ms());
                // Nobody listens until the bot is configured; that is fine.
                let _ = notices.send(Notice::Power(event));
            }
        }
    })?;
    Ok(())
}

fn count_low_samples(input: &PinDriver<'_, Input>) -> u32 {
    (0..WINDOW_SAMPLES)
        .filter(|_| {
            let low = input.is_low();
            FreeRtos::delay_ms(1);
            low
        })
        .count() as u32
}
