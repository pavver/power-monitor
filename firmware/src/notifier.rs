//! Delivers notices (power events, new addresses) to Telegram in order,
//! retrying while offline or until the bot is configured.

use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use power_core::message::{needs_bms, notice_text, Notice};
use power_core::outbox::Outbox;

use crate::bms::Bms;
use crate::settings::Settings;
use crate::status::{now_ms, Status};
use crate::telegram;

const RETRY: Duration = Duration::from_secs(15);
const OUTBOX_CAPACITY: usize = 32;
/// Connecting to the BMS alone may take up to half a minute.
const BMS_WAIT: Duration = Duration::from_secs(45);

pub fn spawn(
    notices: Receiver<Notice>,
    settings: Arc<Settings>,
    status: Arc<Status>,
    bms: Bms,
) -> anyhow::Result<()> {
    // TLS handshakes need a large stack.
    thread::Builder::new().stack_size(12 * 1024).spawn(move || {
        let mut outbox = Outbox::new(OUTBOX_CAPACITY);
        loop {
            if outbox.is_empty() {
                let Ok(notice) = notices.recv() else { return };
                outbox.push(notice);
            }
            notices.try_iter().for_each(|notice| outbox.push(notice));
            status.set_pending(outbox.len());
            if !deliver_next(&mut outbox, &settings, &status, &bms) {
                thread::sleep(RETRY);
            }
        }
    })?;
    Ok(())
}

/// Sends the oldest pending notice; `false` when it has to be retried.
fn deliver_next(outbox: &mut Outbox<Notice>, settings: &Settings, status: &Status, bms: &Bms) -> bool {
    let Some(notice) = outbox.next() else { return true };
    let Some(bot) = settings.bot() else {
        status.set_telegram("бот не налаштовано, повідомлення чекають");
        return false;
    };
    let messages = settings.messages();
    // The BMS is read only for a text that shows its values.
    let battery = if needs_bms(&messages, notice) && settings.battery().is_some() {
        bms.fresh_reading(BMS_WAIT)
    } else {
        None
    };
    let text = notice_text(&messages, notice, now_ms(), battery.as_ref());
    match telegram::send(&bot, &text) {
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
