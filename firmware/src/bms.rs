//! The JK BMS over Bluetooth LE, kept connected while one is saved: it
//! streams its values about once a second, so a message takes the last ones
//! and never waits for the BMS. One thread owns the radio link; everyone
//! else talks to it through `Bms`. The Bluetooth stack starts on first demand.

use std::future::{poll_fn, Future};
use std::pin::pin;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::task::Poll;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use esp32_nimble::utilities::BleUuid;
use esp32_nimble::{BLEAddress, BLEAddressType, BLEClient, BLEDevice, BLEScan};
use esp_idf_svc::hal::task::block_on;
use esp_idf_svc::timer::EspTaskTimerService;
use power_core::jk::{self, Assembler, DeviceInfo, Found, Frame, Reading, Request};
use power_core::settings::Battery;
use serde::Serialize;

use crate::settings::Settings;

/// How long one request for values keeps the link up.
const WANTED_FOR: Duration = Duration::from_secs(20);
const SCAN_MS: i32 = 5_000;
/// The BMS answers a request, and streams cell info, within about a second.
const ANSWER: Duration = Duration::from_secs(5);
const RETRY: Duration = Duration::from_secs(5);
/// Asking again this often keeps the link up for good (see `WANTED_FOR`).
const KEEP_UP: Duration = Duration::from_secs(10);
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
    wanted_until: Option<Instant>,
    scan_asked: bool,
    scanning: bool,
    found: Vec<Found>,
    link: Link,
    error: Option<String>,
    device: Option<DeviceInfo>,
    reading: Option<(Instant, Reading)>,
}

impl State {
    fn wanted(&self) -> bool {
        self.wanted_until.is_some_and(|until| Instant::now() < until)
    }
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
            thread::Builder::new().stack_size(8 * 1024).spawn(move || block_on(worker.run()))?;
            let keeper = bms.clone();
            thread::Builder::new().stack_size(3 * 1024).spawn(move || keeper.keep_up())?;
        }
        Ok(bms)
    }

    /// What the page shows; asking keeps (or brings) the link up for a while.
    pub fn view(&self) -> serde_json::Value {
        let battery = self.settings.battery();
        let state = if battery.is_some() { self.want() } else { self.lock() };
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

    /// Keeps the saved BMS connected.
    fn keep_up(self) {
        loop {
            if self.settings.battery().is_some() {
                drop(self.want());
            }
            thread::sleep(KEEP_UP);
        }
    }

    /// The last values read, however old; does not bring the link up.
    pub fn last_reading(&self) -> Option<Reading> {
        self.lock().reading.as_ref().map(|(_, reading)| reading.clone())
    }

    /// The saved BMS changed: what was read belongs to the old one.
    pub fn forget(&self) {
        self.update(|state| {
            state.device = None;
            state.reading = None;
            state.error = None;
        });
        self.stop_connecting();
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

    fn want(&self) -> MutexGuard<'_, State> {
        let mut state = self.lock();
        state.wanted_until = Some(Instant::now() + WANTED_FOR);
        self.shared.changed.notify_all();
        state
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
            if state.wanted() {
                if let Some(battery) = self.settings.battery() {
                    return Job::Read(battery);
                }
            }
            state.link = Link::Off;
            state = self.shared.changed.wait(state).unwrap();
        }
    }

    /// Whether the link should stay up for `battery`.
    fn keep(&self, battery: &Battery) -> bool {
        let state = self.lock();
        state.wanted() && !state.scan_asked && self.settings.battery().as_ref() == Some(battery)
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
                // Not straight back at a BMS that is out of reach.
                let state = self.lock();
                drop(self.shared.changed.wait_timeout_while(state, RETRY, |s| !s.scan_asked).unwrap());
            }
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
        self.update(|state| {
            state.device = Some(device);
            state.link = Link::Connected;
        });

        // From here on the BMS sends cell info about once a second.
        command(client, Request::CellInfo).await?;
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

/// Connects, subscribes and asks who the BMS is.
async fn open(client: &mut BLEClient, battery: &Battery) -> Result<(Receiver<Frame>, DeviceInfo), String> {
    let kind = if battery.random { BLEAddressType::Random } else { BLEAddressType::Public };
    let address = BLEAddress::from_be_bytes(battery.address, kind);
    client.connect(&address).await.map_err(|e| format!("не вдалося з'єднатися ({e:?})"))?;
    let frames = subscribe(client).await?;
    command(client, Request::DeviceInfo).await?;
    let device = wait_for(&frames, |frame| match frame {
        Frame::DeviceInfo(info) => Some(info),
        _ => None,
    })
    .ok_or("BMS не відповіла на запит")?;
    Ok((frames, device))
}

/// `work`, unless `limit` runs out first.
async fn within<T>(limit: Duration, work: impl Future<Output = T>) -> Option<T> {
    let mut timer = EspTaskTimerService::new().and_then(|service| service.timer_async()).ok()?;
    let mut work = pin!(work);
    let mut alarm = pin!(timer.after(limit));
    poll_fn(|cx| match work.as_mut().poll(cx) {
        Poll::Ready(value) => Poll::Ready(Some(value)),
        Poll::Pending => alarm.as_mut().poll(cx).map(|_| None),
    })
    .await
}

/// Ends the link, or the attempt at one, and waits until the stack has let
/// go of it: connecting again before that fails, and the stack calls back
/// into the client until then.
fn disconnect(client: &mut BLEClient) {
    if client.connected() {
        let _ = client.disconnect();
    } else {
        // SAFETY: NimBLE takes its own lock; without an attempt it only
        // returns an error.
        unsafe { esp_idf_svc::sys::ble_gap_conn_cancel() };
    }
    let deadline = Instant::now() + ANSWER;
    // SAFETY: only reads whether a connect procedure is under way.
    while (client.connected() || unsafe { esp_idf_svc::sys::ble_gap_conn_active() } != 0) && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(50));
    }
}

fn uuid16(uuid: u16) -> BleUuid {
    BleUuid::from_uuid16(uuid)
}

/// Starts the notifications and returns where their frames arrive.
async fn subscribe(client: &mut BLEClient) -> Result<Receiver<Frame>, String> {
    let (sender, frames) = mpsc::channel();
    let mut assembler = Assembler::default();
    // A BMS still holding its last link (just after a restart) lists nothing.
    let listed = client.get_services().await.map_err(|e| format!("сервіси не прочитано ({e:?})"))?.next().is_some();
    if !listed {
        return Err("BMS ще не готова: не дала список сервісів".into());
    }
    let service = client.get_service(uuid16(jk::SERVICE)).await.map_err(|_| "це не JK BMS: немає сервісу FFE0")?;
    // Newer modules split FFE1 in two: one to write to, one that notifies.
    let notifier = service
        .get_characteristics()
        .await
        .map_err(|e| format!("{e:?}"))?
        .find(|c| c.uuid() == uuid16(jk::CHARACTERISTIC) && c.can_notify())
        .ok_or("BMS не надсилає даних")?;
    notifier
        .on_notify(move |chunk| {
            if let Some(frame) = assembler.push(chunk) {
                let _ = sender.send(frame);
            }
        })
        .subscribe_notify(false)
        .await
        .map_err(|e| format!("підписка не вдалася ({e:?})"))?;
    Ok(frames)
}

async fn command(client: &mut BLEClient, request: Request) -> Result<(), String> {
    let service = client.get_service(uuid16(jk::SERVICE)).await.map_err(|e| format!("{e:?}"))?;
    let writer = service
        .get_characteristics()
        .await
        .map_err(|e| format!("{e:?}"))?
        .find(|c| c.uuid() == uuid16(jk::CHARACTERISTIC) && (c.can_write_no_response() || c.can_write()))
        .ok_or("BMS не приймає запитів")?;
    writer.write_value(&jk::command(request), false).await.map_err(|e| format!("запит не надіслано ({e:?})"))
}

/// The first frame `pick` takes, skipping others, within `ANSWER`.
fn wait_for<T>(frames: &Receiver<Frame>, mut pick: impl FnMut(Frame) -> Option<T>) -> Option<T> {
    let deadline = Instant::now() + ANSWER;
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        match frames.recv_timeout(left) {
            Ok(frame) => {
                if let Some(value) = pick(frame) {
                    return Some(value);
                }
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return None,
        }
    }
}

async fn scan() -> Vec<Found> {
    let device = BLEDevice::take();
    let mut heard = Vec::new();
    let result = BLEScan::new()
        .active_scan(true)
        .start(device, SCAN_MS, |seen, data| {
            let address = seen.addr();
            heard.push(Found {
                name: data.name().map(|n| n.to_string()).unwrap_or_default(),
                address: address.to_string().to_uppercase(),
                random: matches!(address.addr_type(), BLEAddressType::Random | BLEAddressType::RandomID),
                rssi: seen.rssi(),
                service: data.is_advertising_service(&uuid16(jk::SERVICE)),
            });
            None::<()>
        })
        .await;
    if let Err(error) = result {
        log::warn!("bms scan: {error:?}");
    }
    jk::candidates(heard)
}
