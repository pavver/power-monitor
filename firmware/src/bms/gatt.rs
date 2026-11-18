//! The JK BMS's GATT side through NimBLE: connecting, notifications,
//! requests and the search.

use std::future::{poll_fn, Future};
use std::pin::pin;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::task::Poll;
use std::thread;
use std::time::{Duration, Instant};

use esp32_nimble::utilities::BleUuid;
use esp32_nimble::{BLEAddress, BLEAddressType, BLEClient, BLEDevice, BLEScan};
use esp_idf_svc::timer::EspTaskTimerService;
use power_core::jk::{self, Assembler, DeviceInfo, Found, Frame, Request};
use power_core::settings::Battery;

use super::ANSWER;

const SCAN_MS: i32 = 5_000;

/// Connects, subscribes and asks who the BMS is.
pub(super) async fn open(client: &mut BLEClient, battery: &Battery) -> Result<(Receiver<Frame>, DeviceInfo), String> {
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
pub(super) async fn within<T>(limit: Duration, work: impl Future<Output = T>) -> Option<T> {
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
pub(super) fn disconnect(client: &mut BLEClient) {
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

pub(super) fn uuid16(uuid: u16) -> BleUuid {
    BleUuid::from_uuid16(uuid)
}

/// Starts the notifications and returns where their frames arrive.
pub(super) async fn subscribe(client: &mut BLEClient) -> Result<Receiver<Frame>, String> {
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
        // The CCCD is written as a request, as the spec asks: a 28:D4:… module
        // ignores a write command there and never notifies.
        .subscribe_notify(true)
        .await
        .map_err(|e| format!("підписка не вдалася ({e:?})"))?;
    Ok(frames)
}

pub(super) async fn command(client: &mut BLEClient, request: Request) -> Result<(), String> {
    let service = client.get_service(uuid16(jk::SERVICE)).await.map_err(|e| format!("{e:?}"))?;
    let writer = service
        .get_characteristics()
        .await
        .map_err(|e| format!("{e:?}"))?
        .find(|c| c.uuid() == uuid16(jk::CHARACTERISTIC) && (c.can_write_no_response() || c.can_write()))
        .ok_or("BMS не приймає запитів")?;
    // Without a reply, as ESPHome sends them: a BD6A20S10P (19.20) never
    // answers a write request, which would leave the worker waiting.
    writer.write_value(&jk::command(request), false).await.map_err(|e| format!("запит не надіслано ({e:?})"))
}

/// The first frame `pick` takes, skipping others, within `ANSWER`.
pub(super) fn wait_for<T>(frames: &Receiver<Frame>, mut pick: impl FnMut(Frame) -> Option<T>) -> Option<T> {
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

pub(super) async fn scan() -> Vec<Found> {
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
