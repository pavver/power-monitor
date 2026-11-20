//! Keeps the live status message in the chat and sends the automations'
//! messages: ticks once a second, waits for Wi-Fi and the bot, retries
//! failed sends.

use std::fmt;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Duration;

use autotext::automation::{Action, Automation, Engine, LocalTime, Now};
use autotext::status::{Status as LiveStatus, Step};
use autotext::template::render_with;
use power_core::api::Link;
use power_core::message::Notice;
use power_core::outbox::Outbox;
use power_core::power::PowerState;
use power_core::settings::Bot;
use power_core::telegram::Outcome;
use power_core::texts::{self, Values};

use crate::bms::Bms;
use crate::settings::Settings;
use crate::status::{now_ms, Status};
use crate::telegram;
use crate::voltmeter::Voltmeter;

const RETRY_MS: u64 = 15_000;
const TICK: Duration = Duration::from_secs(1);
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
    crate::tasks::spawn("notifier", 7 * 1024, move || {
        let mut notifier = Notifier::new(settings, status, bms, voltmeter, kept);
        loop {
            notifier.tick(&notices);
            std::thread::sleep(TICK);
        }
    })?;
    Ok(())
}

struct Notifier {
    settings: Arc<Settings>,
    status: Arc<Status>,
    bms: Bms,
    voltmeter: Voltmeter,
    /// What the sensor said last; `None` until its first confirmed state.
    mains: Option<PowerState>,
    /// Starts as the address last announced, so only a new one is a change.
    ip: Option<String>,
    engine: Engine,
    /// The automations' generation the engine was made for.
    generation: Option<u32>,
    outbox: Outbox<String>,
    live: LiveStatus,
    /// Messages counted as sent when the status last went out: any more
    /// (a test, an automation) and it is no longer the bottom one.
    seen_sent: u32,
    retry_at_ms: u64,
    /// The panel says "waiting for Wi-Fi".
    waiting: bool,
}

impl Notifier {
    fn new(settings: Arc<Settings>, status: Arc<Status>, bms: Bms, voltmeter: Voltmeter, kept: Option<PowerState>) -> Self {
        Self {
            live: LiveStatus::new(settings.live()),
            ip: settings.last_ip(),
            settings,
            status,
            bms,
            voltmeter,
            mains: kept,
            engine: Engine::default(),
            generation: None,
            outbox: Outbox::new(OUTBOX_CAPACITY),
            seen_sent: telegram::sent_count(),
            retry_at_ms: 0,
            waiting: false,
        }
    }

    fn tick(&mut self, notices: &Receiver<Notice>) {
        notices.try_iter().for_each(|notice| self.take(notice));
        // Taken once: the two passes of a streamed request render the same text.
        let values = self.values();
        let values = values.as_slice();
        self.run_automations(&values);
        if telegram::sent_count() != self.seen_sent {
            self.live.other_sent();
            self.seen_sent = telegram::sent_count();
        }
        self.status.set_pending(self.outbox.len());
        let Some(bot) = self.ready() else { return };
        let result = match self.outbox.next().cloned() {
            Some(message) => self.send_queued(&bot, &message),
            None if self.mains.is_some() => self.update_status(&bot, &values),
            // No status before the sensor has said whether there is power.
            None => Ok(()),
        };
        if let Err(error) = result {
            log::warn!("telegram: {error}");
            self.status.set_telegram(format!("помилка: {error}"));
            self.retry_at_ms = now_ms() + RETRY_MS;
        }
    }

    fn take(&mut self, notice: Notice) {
        match notice {
            Notice::Power(event) => {
                self.mains = Some(event.state);
                if self.settings.status_config().important.iter().any(|name| name == "mains") {
                    self.live.important_changed();
                }
            }
            Notice::Address(ip) => self.ip = Some(ip),
        }
    }

    fn values(&self) -> Values {
        let device = self.settings.with_messages(|messages| messages.device_name().to_string());
        let reading = self.settings.battery().and_then(|_| self.bms.recent_reading(BMS_MAX_AGE));
        texts::values(&texts::Snapshot {
            device: &device,
            mains: self.mains,
            ip: self.ip.as_deref(),
            voltage_mv: self.voltmeter.battery_mv(),
            bms: reading.as_ref(),
            local: crate::clock::local(),
        })
    }

    fn run_automations(&mut self, values: &[(&str, Option<&str>)]) {
        let (automations, generation) = self.settings.automations_with_gen();
        if self.generation != Some(generation) {
            // The user edited the list: memories of the old one mean nothing.
            self.engine = Engine::new(automations.len());
            self.generation = Some(generation);
        }
        let local = crate::clock::local().map(|l| LocalTime { day: l.days, minute: l.minute_of_day() });
        for index in self.engine.tick(&automations, Now { ms: now_ms(), local }, values) {
            self.act(&automations[index], values);
        }
    }

    fn act(&mut self, automation: &Automation, values: &[(&str, Option<&str>)]) {
        match automation.action {
            Action::Send => {
                let mut text = String::new();
                let _ = render_with(&automation.text, values, &*self.settings, &mut text);
                self.outbox.push(text);
            }
            Action::UpdateStatus => self.live.want_update(),
        }
    }

    /// The bot, when there is one, Wi-Fi is up and no retry is waited for.
    fn ready(&mut self) -> Option<Bot> {
        let Some(bot) = self.settings.bot() else {
            self.status.set_telegram("бот не налаштовано, повідомлення чекають");
            return None;
        };
        if !matches!(self.status.link(), Link::Connected { .. }) {
            self.status.set_telegram("чекає на Wi-Fi");
            self.waiting = true;
            return None;
        }
        if std::mem::take(&mut self.waiting) {
            // Otherwise "waiting for Wi-Fi" stays until the next message goes out.
            self.status.set_telegram("на зв'язку");
        }
        (now_ms() >= self.retry_at_ms).then_some(bot)
    }

    fn send_queued(&mut self, bot: &Bot, message: &str) -> Result<(), String> {
        telegram::send(bot, message)?;
        self.outbox.delivered();
        self.status.set_pending(self.outbox.len());
        self.status.set_telegram("останнє повідомлення доставлено");
        Ok(())
    }

    fn update_status(&mut self, bot: &Bot, values: &[(&str, Option<&str>)]) -> Result<(), String> {
        let config = self.settings.status_config();
        let step = self.live.decide(&config, now_ms(), values);
        if step == Step::Wait {
            return Ok(());
        }
        // Read only now: most ticks wait, and NVS is not read every second.
        let template = self.settings.template(texts::STATUS_TEMPLATE).unwrap_or_default();
        let settings = self.settings.clone();
        let render = |out: &mut dyn fmt::Write| render_with(&template, values, &*settings, out);
        match step {
            Step::Wait => return Ok(()),
            Step::Edit(id) => match telegram::edit_with(bot, id, render)? {
                Outcome::Done => {
                    self.live.edited(now_ms());
                    self.status.set_telegram("повідомлення оновлено");
                    return Ok(());
                }
                // Deleted in the chat: the next tick sends a new one.
                Outcome::Gone => {
                    self.live.gone();
                    return self.settings.save_live(None).map_err(|e| e.to_string());
                }
            },
            Step::Replace(id) => {
                telegram::delete(bot, id)?;
            }
            Step::Send => {}
        }
        let id = telegram::send_with(bot, render)?;
        self.live.sent(id, now_ms(), &config, values);
        self.seen_sent = telegram::sent_count();
        self.status.set_telegram("останнє повідомлення доставлено");
        self.settings.save_live(self.live.live()).map_err(|e| e.to_string())
    }
}
