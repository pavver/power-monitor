//! The JK BMS over Bluetooth LE, kept connected while one is saved: it
//! streams its values about once a second, so a message takes the last ones
//! and never waits for the BMS. One thread owns the radio link; everyone
//! else talks to it through `Bms`. The Bluetooth stack starts on first demand.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use esp32_nimble::{BLEClient, BLEDevice};
use esp_idf_svc::hal::task::block_on;
use power_core::jk::{self, DeviceInfo, Found, Frame, Reading, Request};
use power_core::settings::Battery;
use serde::Serialize;

use crate::settings::Settings;

mod gatt;
use gatt::{command, disconnect, open, scan, wait_for, within};

/// The BMS answers a request, and streams cell info, within about a second.
const ANSWER: Duration = Duration::from_secs(5);
const RETRY: Duration = Duration::from_secs(5);
/// Connecting (up to 30 s) and the first answers. NimBLE waits for some
/// replies with no limit of its own, e.g. the MTU exchange of a BMS that
/// stopped answering.
const SETUP: Duration = Duration::from_secs(40);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "lowercase")]
enum Link {
    #[default]
    Off,
    Connecting,
    Connected,
    Failed,
}

#[derive(Default)]
struct State {
    scan_asked: bool,
    scanning: bool,
    found: Vec<Found>,
    link: Link,
    error: Option<String>,
    device: Option<DeviceInfo>,
    reading: Option<(Instant, Reading)>,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

enum Job {
    Scan,
    Read(Battery),
}

#[derive(Clone)]
pub struct Bms {
    shared: Arc<Shared>,
    settings: Arc<Settings>,
    /// Whether this start kept Bluetooth (see main::keep_bluetooth).
    bluetooth: bool,
}

impl Bms {
    /// Without `bluetooth` nothing is ever read: a search needs a restart.
    pub fn start(settings: Arc<Settings>, bluetooth: bool) -> anyhow::Result<Self> {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), changed: Condvar::new() });
        let bms = Self { shared, settings, bluetooth };
        if bluetooth {
            let worker = bms.clone();
            crate::tasks::spawn("bms", 8 * 1024, move || block_on(worker.run()))?;
        }
        Ok(bms)
    }

    /// What the page shows.
    pub fn view(&self) -> serde_json::Value {
        let battery = self.settings.battery();
        let state = self.lock();
        let reading = state.reading.as_ref().map(|(at, reading)| {
            serde_json::json!({
                "age_s": at.elapsed().as_secs(),
                "values": reading.values().into_iter().collect::<std::collections::BTreeMap<_, _>>(),
            })
        });
        serde_json::json!({
            "battery": battery.map(|b| serde_json::json!({ "name": b.name, "address": b.address_text() })),
            "bluetooth": self.bluetooth,
            "link": state.link,
            "error": state.error,
            "device": state.device,
            "reading": reading,
            "scanning": state.scanning || state.scan_asked,
            "found": state.found.iter().map(|f| serde_json::json!({
                "name": f.name, "address": f.address, "random": f.random, "rssi": f.rssi, "likely": f.likely(),
            })).collect::<Vec<_>>(),
        })
    }

    /// Looks for devices nearby; the result shows up in `view`. `false`:
    /// Bluetooth is off until a restart.
    pub fn scan(&self) -> bool {
        if self.bluetooth {
            self.update(|state| state.scan_asked = true);
            self.stop_connecting();
        }
        self.bluetooth
    }

    /// The last values read, if not older than `max_age`; never waits.
    pub fn recent_reading(&self, max_age: Duration) -> Option<Reading> {
        let state = self.lock();
        let (at, reading) = state.reading.as_ref()?;
        (at.elapsed() <= max_age).then(|| reading.clone())
    }

    /// The saved BMS changed: what was read belongs to the old one. The
    /// link shows what comes next, not what the old session still holds.
    pub fn forget(&self) {
        self.stop_connecting();
        let link = if self.settings.battery().is_some() { Link::Connecting } else { Link::Off };
        self.update(|state| {
            state.device = None;
            state.reading = None;
            state.error = None;
            state.link = link;
        });
    }

    /// A BMS that is out of reach, or busy with another client, holds the
    /// link in `connect` for up to 30 s; nothing else gets the radio then.
    fn stop_connecting(&self) {
        if self.lock().link == Link::Connecting {
            // SAFETY: NimBLE takes its own lock; the stack is up while a
            // connection is attempted. Without one it only returns an error.
            unsafe { esp_idf_svc::sys::ble_gap_conn_cancel() };
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap()
    }

    fn update(&self, change: impl FnOnce(&mut State)) {
        change(&mut self.lock());
        self.shared.changed.notify_all();
    }

    async fn run(self) {
        loop {
            match self.next_job() {
                Job::Scan => {
                    let found = scan().await;
                    self.update(|state| {
                        state.scanning = false;
                        state.found = found;
                    });
                }
                Job::Read(battery) => {
                    // A fresh client each time: one keeps the services it
                    // found, even an empty list from a discovery cut short.
                    let mut client = BLEDevice::take().new_client();
                    self.session(&mut client, &battery).await;
                }
            }
        }
    }

    /// Sleeps until there is something to do.
    fn next_job(&self) -> Job {
        let mut state = self.lock();
        loop {
            if state.scan_asked {
                state.scan_asked = false;
                state.scanning = true;
                return Job::Scan;
            }
            // A saved BMS is kept connected; saving one wakes this up.
            if let Some(battery) = self.settings.battery() {
                return Job::Read(battery);
            }
            state.link = Link::Off;
            state = self.shared.changed.wait(state).unwrap();
        }
    }

    /// Whether the link should stay up for `battery`.
    fn keep(&self, battery: &Battery) -> bool {
        let state = self.lock();
        !state.scan_asked && self.settings.battery().as_ref() == Some(battery)
    }

    async fn session(&self, client: &mut BLEClient, battery: &Battery) {
        self.update(|state| {
            state.link = Link::Connecting;
            state.error = None;
        });
        let result = self.serve(client, battery).await;
        disconnect(client);
        if let Err(error) = &result {
            log::warn!("bms {}: {error}", battery.address_text());
        }
        match result {
            // A BMS forgotten, or put aside for a search, has not failed.
            Err(error) if self.keep(battery) => {
                self.update(|state| {
                    state.link = Link::Failed;
                    state.error = Some(error);
                });
                // Not straight back at a BMS that is out of reach; a search
                // or another BMS chosen does not wait.
                let state = self.lock();
                let same = || self.settings.battery().as_ref() == Some(battery);
                drop(self.shared.changed.wait_timeout_while(state, RETRY, |s| !s.scan_asked && same()).unwrap());
            }
            // Another BMS chosen: `forget` has set the link for it already.
            _ if self.settings.battery().as_ref() != Some(battery) => {}
            _ => self.update(|state| state.link = Link::Off),
        }
    }

    async fn serve(&self, client: &mut BLEClient, battery: &Battery) -> Result<(), String> {
        if !self.keep(battery) {
            return Ok(());
        }
        let (frames, device) = within(SETUP, open(client, battery)).await.ok_or("BMS не відповіла вчасно")??;
        let layout = device.layout();
        log::info!("bms: {} hw {} sw {}, {layout:?}", device.model, device.hardware, device.software);
        {
            let mut state = self.lock();
            if self.settings.battery().as_ref() != Some(battery) {
                return Ok(());
            }
            state.device = Some(device);
            state.link = Link::Connected;
        }
        self.shared.changed.notify_all();

        // From here on the BMS sends cell info about once a second.
        within(ANSWER, command(client, Request::CellInfo)).await.ok_or("BMS не прийняла запит")??;
        while self.keep(battery) {
            if !client.connected() {
                return Err("з'єднання обірвалось".into());
            }
            let Some(bytes) = wait_for(&frames, |frame| match frame {
                Frame::CellInfo(bytes) => Some(bytes),
                _ => None,
            }) else {
                // Some BMS (seen on 19.13) end the stream after half a minute
                // and ignore a new request; a fresh connection starts it again.
                log::info!("bms {}: stream ended, reconnecting", battery.address_text());
                return Ok(());
            };
            let reading = jk::reading(&bytes, layout);
            // Checked under the lock: a BMS forgotten meanwhile has its say no more.
            let mut state = self.lock();
            if self.settings.battery().as_ref() != Some(battery) {
                break;
            }
            state.reading = Some((Instant::now(), reading));
            drop(state);
            self.shared.changed.notify_all();
        }
        Ok(())
    }
}
