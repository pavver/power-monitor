//! Wall-clock time and local time zone.

use std::sync::Mutex;

use esp_idf_svc::sntp::EspSntp;
use power_core::clock::{self, Local, TimeZone};

static SNTP: Mutex<Option<EspSntp<'static>>> = Mutex::new(None);

/// Starts SNTP synchronization with default servers once Wi-Fi is up.
pub fn start_sntp() {
    let mut guard = SNTP.lock().unwrap();
    if guard.is_none() {
        match EspSntp::new_default() {
            Ok(sntp) => {
                log::info!("sntp: started");
                *guard = Some(sntp);
            }
            Err(e) => {
                log::warn!("sntp start error: {e}");
            }
        }
    }
}

/// Applies the time zone to libc via setenv("TZ", ...) and tzset().
pub fn apply(tz: &TimeZone) {
    if let Ok(tz_c) = std::ffi::CString::new(tz.as_str()) {
        const TZ_VAR: &[u8] = b"TZ\0";
        // SAFETY: calling C libc setenv and tzset via esp_idf_svc::sys
        unsafe {
            esp_idf_svc::sys::setenv(TZ_VAR.as_ptr().cast(), tz_c.as_ptr(), 1);
            esp_idf_svc::sys::tzset();
        }
        log::info!("clock: time zone applied: {tz}");
    }
}

/// Current local time, or `None` if the clock has not been set (e.g. before SNTP).
pub fn local() -> Option<Local> {
    let mut now: esp_idf_svc::sys::time_t = 0;
    // SAFETY: time() fills `now`.
    unsafe { esp_idf_svc::sys::time(&mut now) };
    if !clock::is_set(now as i64) {
        return None;
    }
    let mut tm = unsafe { std::mem::zeroed::<esp_idf_svc::sys::tm>() };
    // SAFETY: localtime_r converts `now` into local broken-down time in `tm`.
    let res = unsafe { esp_idf_svc::sys::localtime_r(&now, &mut tm) };
    if res.is_null() {
        return None;
    }
    Some(Local::new(
        tm.tm_year + 1900,
        (tm.tm_mon + 1) as u8,
        tm.tm_mday as u8,
        tm.tm_hour as u8,
        tm.tm_min as u8,
    ))
}
