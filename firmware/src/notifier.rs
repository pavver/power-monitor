//! Delivers notices (power events, reminders, new addresses) to Telegram in
//! order, waiting for Wi-Fi and the bot, and retrying failed sends.

use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use power_core::api::Link;
use power_core::message::{needs_bms, notice_text, Notice, Readings};
use power_core::outbox::Outbox;

use crate::bms::Bms;
use crate::settings::Settings;
use crate::status::{now_ms, Status};
use crate::telegram;
use crate::voltmeter::Voltmeter;

const RETRY: Duration = Duration::from_secs(15);
/// How often to look whether Wi-Fi is back, so a message goes out right after.
const LINK_CHECK: Duration = Duration::from_secs(1);
const OUTBOX_CAPACITY: usize = 32;
/// Battery values this old still go into a message. The BMS streams them
/// every second while connected; older ones mean the link is down.
pub const BMS_MAX_AGE: Duration = Duration::from_secs(60);

pub fn spawn(
    notices: Receiver<Notice>,
    settings: Arc<Settings>,
    status: Arc<Status>,
    bms: Bms,
    voltmeter: Voltmeter,
) -> anyhow::Result<()> {
    // TLS handshakes need a large stack.
    crate::tasks::spawn(c"notifier", 12 * 1024, move || {
        let mut outbox = Outbox::new(OUTBOX_CAPACITY);
        loop {
            if outbox.is_empty() {
                let Ok(notice) = notices.recv() else { return };
                queue(&mut outbox, notice);
            }
            notices.try_iter().for_each(|notice| queue(&mut outbox, notice));
            status.set_pending(outbox.len());
            match deliver_next(&mut outbox, &settings, &status, &bms, &voltmeter) {
                Delivery::Done => {}
                Delivery::Offline => thread::sleep(LINK_CHECK),
                Delivery::Failed => thread::sleep(RETRY),
            }
        }
    })?;
    Ok(())
}

/// Queues `notice` in place of what it makes old news.
fn queue(outbox: &mut Outbox<Notice>, notice: Notice) {
    outbox.retain(|older| !notice.outdates(older));
    outbox.push(notice);
}

enum Delivery {
    /// Sent, or nothing to send.
    Done,
    /// No Wi-Fi yet: no point trying.
    Offline,
    /// Try again later.
    Failed,
}

/// Sends the oldest pending notice.
fn deliver_next(
    outbox: &mut Outbox<Notice>,
    settings: &Settings,
    status: &Status,
    bms: &Bms,
    voltmeter: &Voltmeter,
) -> Delivery {
    let Some(notice) = outbox.next() else { return Delivery::Done };
    let Some(bot) = settings.bot() else {
        status.set_telegram("бот не налаштовано, повідомлення чекають");
        return Delivery::Failed;
    };
    if !matches!(status.link(), Link::Connected { .. }) {
        status.set_telegram("чекає на Wi-Fi");
        return Delivery::Offline;
    }
    // Never waits for the BMS: a value too old to show is a shrug.
    let battery = if settings.with_messages(|messages| needs_bms(messages, notice)) && settings.battery().is_some() {
        bms.recent_reading(BMS_MAX_AGE)
    } else {
        None
    };
    let readings = Readings { bms: battery.as_ref(), voltage_mv: voltmeter.battery_mv() };
    let Some(text) = settings.with_messages(|messages| notice_text(messages, notice, now_ms(), readings)) else {
        // Its text is empty: the user turned this message off.
        outbox.delivered();
        status.set_pending(outbox.len());
        status.set_telegram("останнє повідомлення вимкнене (порожній текст), не надсилалось");
        return Delivery::Done;
    };
    match telegram::send(&bot, &text) {
        Ok(()) => {
            outbox.delivered();
            status.set_pending(outbox.len());
            status.set_telegram("останнє повідомлення доставлено");
            Delivery::Done
        }
        Err(error) => {
            log::warn!("telegram: {error}");
            status.set_telegram(format!("помилка: {error}"));
            Delivery::Failed
        }
    }
}
