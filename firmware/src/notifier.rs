//! Delivers power events to Telegram in order, retrying while offline.

use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use power_core::config::Config;
use power_core::message::event_text;
use power_core::outbox::Outbox;
use power_core::power::PowerEvent;

use crate::status::{now_ms, Status};
use crate::telegram;

const RETRY: Duration = Duration::from_secs(15);
const OUTBOX_CAPACITY: usize = 32;

pub fn spawn(config: Config, events: Receiver<PowerEvent>, status: Arc<Status>) -> anyhow::Result<()> {
    // TLS handshakes need a large stack.
    thread::Builder::new().stack_size(12 * 1024).spawn(move || {
        let mut outbox = Outbox::new(OUTBOX_CAPACITY);
        loop {
            if outbox.is_empty() {
                let Ok(event) = events.recv() else { return };
                outbox.push(event);
            }
            events.try_iter().for_each(|event| outbox.push(event));
            status.set_pending(outbox.len());
            if !deliver_next(&config, &mut outbox, &status) {
                thread::sleep(RETRY);
            }
        }
    })?;
    Ok(())
}

/// Sends the oldest pending event; `false` when it has to be retried.
fn deliver_next(config: &Config, outbox: &mut Outbox, status: &Status) -> bool {
    let Some(event) = outbox.next() else { return true };
    let text = event_text(config.device_name(), event, now_ms());
    match telegram::send(config, &text) {
        Ok(()) => {
            outbox.delivered();
            status.set_pending(outbox.len());
            status.set_telegram("останнє повідомлення доставлено");
            true
        }
        Err(error) => {
            log::warn!("telegram: {error}");
            status.set_telegram(format!("помилка: {error}"));
            false
        }
    }
}
