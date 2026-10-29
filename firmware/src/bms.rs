//! The JK BMS over Bluetooth LE, read only while someone wants its values:
//! the page while the battery screen is open, the notifier for a message
//! that shows them. One thread owns the radio link; everyone else talks to
//! it through `Bms`. The Bluetooth stack itself starts on first demand.

use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use esp32_nimble::utilities::BleUuid;
use esp32_nimble::{BLEAddress, BLEAddressType, BLEClient, BLEDevice, BLEScan};
use esp_idf_svc::hal::task::block_on;
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
}

impl Bms {
    pub fn start(settings: Arc<Settings>) -> anyhow::Result<Self> {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), changed: Condvar::new() });
        let bms = Self { shared, settings };
        let worker = bms.clone();
        thread::Builder::new().stack_size(8 * 1024).spawn(move || block_on(worker.run()))?;
        Ok(bms)
    }

    /// What the page shows; asking keeps (or brings) the link up for a while.
    pub fn view(&self) -> serde_json::Value {
        let battery = self.settings.battery();
        let state = if battery.is_some() { self.want() } else { self.lock() };
        let reading = state.reading.as_ref().map(|(at, reading)| {
            serde_json::json!({
                "age_s": at.elapsed().as_secs(),
                "cells_mv": reading.cells_mv,
                "values": reading.values().into_iter().collect::<std::collections::BTreeMap<_, _>>(),
            })
        });
        serde_json::json!({
            "battery": battery.map(|b| serde_json::json!({ "name": b.name, "address": b.address_text() })),
            "link": state.link,
            "error": state.error,
            "device": state.device,
            "reading": reading,
            "scanning": state.scanning || state.scan_asked,
            "found": state.found,
        })
    }

    /// Looks for devices nearby; the result shows up in `view`.
    pub fn scan(&self) {
        self.update(|state| state.scan_asked = true);
    }

    /// A reading taken after this call, waiting at most `wait` for it.
    pub fn fresh_reading(&self, wait: Duration) -> Option<Reading> {
        let asked = Instant::now();
        let mut state = self.want();
        loop {
            if let Some((at, reading)) = &state.reading {
                if *at >= asked {
                    return Some(reading.clone());
                }
            }
            let left = wait.checked_sub(asked.elapsed())?;
            state = self.shared.changed.wait_timeout(state, left).unwrap().0;
        }
    }

    /// The saved BMS changed: what was read belongs to the old one.
    pub fn forget(&self) {
        self.update(|state| {
            state.device = None;
            state.reading = None;
            state.error = None;
        });
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
        // A client caches the services of the device it talks to, so each
        // BMS gets its own. It must stay in place while connected: the
        // stack keeps a pointer to it.
        let mut client: Option<(Battery, BLEClient)> = None;
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
                    if client.as_ref().is_none_or(|(b, _)| b.address != battery.address) {
                        client = Some((battery.clone(), BLEDevice::take().new_client()));
                    }
                    let (_, client) = client.as_mut().unwrap();
                    self.session(client, &battery).await;
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
        if client.connected() {
            let _ = client.disconnect();
        }
        if let Err(error) = result {
            log::warn!("bms {}: {error}", battery.address_text());
            self.update(|state| {
                state.link = Link::Failed;
                state.error = Some(error);
            });
            // Not straight back at a BMS that is out of reach.
            let state = self.lock();
            drop(self.shared.changed.wait_timeout_while(state, RETRY, |s| !s.scan_asked).unwrap());
        }
    }

    async fn serve(&self, client: &mut BLEClient, battery: &Battery) -> Result<(), String> {
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
            let bytes = wait_for(&frames, |frame| match frame {
                Frame::CellInfo(bytes) => Some(bytes),
                _ => None,
            })
            .ok_or("BMS перестала надсилати дані")?;
            let reading = jk::reading(&bytes, layout);
            self.update(|state| state.reading = Some((Instant::now(), reading)));
        }
        Ok(())
    }
}

fn uuid16(uuid: u16) -> BleUuid {
    BleUuid::from_uuid16(uuid)
}

/// Starts the notifications and returns where their frames arrive.
async fn subscribe(client: &mut BLEClient) -> Result<Receiver<Frame>, String> {
    let (sender, frames) = mpsc::channel();
    let mut assembler = Assembler::default();
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
