//! Delivers notifications and live status updates to Telegram in order,
//! waiting for Wi-Fi and the bot, and retrying failed sends.

use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use autotext::automation::{Action, Engine, LocalTime, Now};
use autotext::status::{Status as LiveStatus, Step};
use power_core::api::Link;
use power_core::message::Notice;
use power_core::outbox::Outbox;
use power_core::power::PowerState;
use power_core::telegram::Outcome;
use power_core::texts;

use crate::bms::Bms;
use crate::settings::Settings;
use crate::status::{now_ms, Status};
use crate::telegram;
use crate::voltmeter::Voltmeter;

const RETRY: Duration = Duration::from_secs(15);
/// How often to tick the loop and check whether Wi-Fi is back.
const LINK_CHECK: Duration = Duration::from_secs(1);
const OUTBOX_CAPACITY: usize = 8;
/// Battery values this old still go into a message. The BMS streams them
/// every second while connected; older ones mean the link is down.
pub const BMS_MAX_AGE: Duration = Duration::from_secs(60);

pub fn spawn(
    notices: Receiver<Notice>,
    settings: Arc<Settings>,
    status: Arc<Status>,
    bms: Bms,
    voltmeter: Voltmeter,
    kept: Option<PowerState>,
) -> anyhow::Result<()> {
    // TLS handshakes need a large stack.
    crate::tasks::spawn("notifier", 12 * 1024, move || {
        let mut outbox: Outbox<String> = Outbox::new(OUTBOX_CAPACITY);
        let mut live_status = LiveStatus::new(settings.live());
        let mut mains: Option<PowerState> = kept;
        let mut ip: Option<String> = settings.last_ip();
        let mut last_gen: u32 = u32::MAX;
        let mut engine = Engine::new(0);
        let mut retry_after: u64 = 0;
        // Messages counted as sent when the status last went out: any more
        // (test, automations) and it is no longer the bottom one.
        let mut seen_sent = telegram::sent_count();

        loop {
            let now = now_ms();

            // 1. Drain notice channel
            while let Ok(notice) = notices.try_recv() {
                match notice {
                    Notice::Power(event) => {
                        mains = Some(event.state);
                        let config = settings.status_config();
                        if config.important.iter().any(|k| k == "mains") {
                            live_status.important_changed();
                        }
                    }
                    Notice::Address(new_ip) => {
                        ip = Some(new_ip);
                    }
                    _ => {}
                }
            }

            // 2. Build Snapshot and Values once per tick
            let device_name = settings.with_messages(|m| m.device_name().to_string());
            let bms_reading = if settings.battery().is_some() {
                bms.recent_reading(BMS_MAX_AGE)
            } else {
                None
            };
            let local = crate::clock::local();
            let snap = texts::Snapshot {
                device: &device_name,
                mains,
                ip: ip.as_deref(),
                voltage_mv: voltmeter.battery_mv(),
                bms: bms_reading.as_ref(),
                local,
            };
            let cur_values = texts::values(&snap);
            let values_slice = cur_values.as_slice();

            // 3. Engine tick with Now
            let (autos, gen) = settings.automations_with_gen();
            if gen != last_gen {
                engine = Engine::new(autos.len());
                last_gen = gen;
            }
            let now_time = Now {
                ms: now,
                local: local.map(|l| LocalTime {
                    day: l.days,
                    minute: l.minute_of_day(),
                }),
            };
            let fired = engine.tick(&autos, now_time, &values_slice);
            for idx in fired {
                let a = &autos[idx];
                match a.action {
                    Action::Send => {
                        let mut rendered = String::new();
                        let _ = autotext::template::render_with(&a.text, &values_slice, &*settings, &mut rendered);
                        outbox.push(rendered);
                    }
                    Action::UpdateStatus => {
                        live_status.want_update();
                    }
                }
            }
            status.set_pending(outbox.len());

            if telegram::sent_count() != seen_sent {
                live_status.other_sent();
                seen_sent = telegram::sent_count();
            }

            // 4. Delivery / Status decision
            let bot = settings.bot();
            let is_connected = matches!(status.link(), Link::Connected { .. });
            match (bot, is_connected) {
                (None, _) => {
                    status.set_telegram("бот не налаштовано, повідомлення чекають");
                }
                (Some(_), false) => {
                    status.set_telegram("чекає на Wi-Fi");
                }
                (Some(bot), true) => {
                    if now >= retry_after {
                        if let Some(msg) = outbox.next() {
                            match telegram::send(&bot, msg) {
                                Ok(_) => {
                                    outbox.delivered();
                                    status.set_pending(outbox.len());
                                    status.set_telegram("останнє повідомлення доставлено");
                                }
                                Err(error) => {
                                    log::warn!("telegram: {error}");
                                    status.set_telegram(format!("помилка: {error}"));
                                    retry_after = now + RETRY.as_millis() as u64;
                                }
                            }
                        } else if mains.is_some() {
                            let config = settings.status_config();
                            let step = live_status.decide(&config, now, &values_slice);
                            match step {
                                Step::Send => {
                                    let status_tpl = settings.template("status").unwrap_or_default();
                                    match telegram::send_with(&bot, |out| {
                                        autotext::template::render_with(&status_tpl, &values_slice, &*settings, out)
                                    }) {
                                        Ok(id) => {
                                            live_status.sent(id, now, &config, &values_slice);
                                            let _ = settings.save_live(live_status.live());
                                            seen_sent = telegram::sent_count();
                                            status.set_telegram("останнє повідомлення доставлено");
                                        }
                                        Err(error) => {
                                            log::warn!("telegram: {error}");
                                            status.set_telegram(format!("помилка: {error}"));
                                            retry_after = now + RETRY.as_millis() as u64;
                                        }
                                    }
                                }
                                Step::Edit(id) => {
                                    let status_tpl = settings.template("status").unwrap_or_default();
                                    match telegram::edit_with(&bot, id, |out| {
                                        autotext::template::render_with(&status_tpl, &values_slice, &*settings, out)
                                    }) {
                                        Ok(Outcome::Done) => {
                                            live_status.edited(now);
                                            status.set_telegram("повідомлення оновлено");
                                        }
                                        Ok(Outcome::Gone) => {
                                            live_status.gone();
                                            let _ = settings.save_live(None);
                                        }
                                        Err(error) => {
                                            log::warn!("telegram: {error}");
                                            status.set_telegram(format!("помилка: {error}"));
                                            retry_after = now + RETRY.as_millis() as u64;
                                        }
                                    }
                                }
                                Step::Replace(id) => {
                                    match telegram::delete(&bot, id) {
                                        Ok(Outcome::Done) | Ok(Outcome::Gone) => {
                                            let status_tpl = settings.template("status").unwrap_or_default();
                                            match telegram::send_with(&bot, |out| {
                                                autotext::template::render_with(&status_tpl, &values_slice, &*settings, out)
                                            }) {
                                                Ok(new_id) => {
                                                    live_status.sent(new_id, now, &config, &values_slice);
                                                    let _ = settings.save_live(live_status.live());
                                            seen_sent = telegram::sent_count();
                                                    status.set_telegram("останнє повідомлення доставлено");
                                                }
                                                Err(error) => {
                                                    log::warn!("telegram: {error}");
                                                    status.set_telegram(format!("помилка: {error}"));
                                                    retry_after = now + RETRY.as_millis() as u64;
                                                }
                                            }
                                        }
                                        Err(error) => {
                                            log::warn!("telegram: {error}");
                                            status.set_telegram(format!("помилка: {error}"));
                                            retry_after = now + RETRY.as_millis() as u64;
                                        }
                                    }
                                }
                                Step::Wait => {}
                            }
                        }
                    }
                }
            }

            thread::sleep(LINK_CHECK);
        }
    })?;
    Ok(())
}
