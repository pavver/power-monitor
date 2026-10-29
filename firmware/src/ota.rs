//! Over-the-air updates: writing an uploaded image to the idle slot, and
//! confirming a freshly booted image so the bootloader does not roll it back.

use embedded_svc::io::Read;
use esp_idf_svc::ota::EspOta;
use esp_idf_svc::sys::esp_ota_get_next_update_partition;
use power_core::api::Firmware;
use power_core::ota::{check_image, ImageInfo, HEADER_LEN};

pub fn running() -> Firmware {
    let slot = EspOta::new().and_then(|ota| ota.get_running_slot());
    match slot {
        Ok(slot) => {
            let (version, built) = slot
                .firmware
                .map_or_else(Default::default, |f| (f.version.to_string(), f.released.to_string()));
            Firmware { version, built, slot: slot.label.to_string() }
        }
        Err(error) => {
            log::warn!("ota slot: {error}");
            Firmware::default()
        }
    }
}

/// With rollback enabled a new image boots on probation; once it is up far
/// enough to accept another update, it is kept.
pub fn confirm_running() {
    if let Err(error) = EspOta::new().and_then(|mut ota| ota.mark_running_slot_valid()) {
        log::warn!("ota confirm: {error}");
    }
}

/// Streams an image of `size` bytes from `body` into the idle slot and makes
/// it the boot slot. Nothing changes unless the whole image is written and valid.
pub fn update<R: Read>(body: &mut R, size: usize) -> Result<ImageInfo, String> {
    let mut header = [0u8; HEADER_LEN];
    let header_len = HEADER_LEN.min(size);
    body.read_exact(&mut header[..header_len]).map_err(|_| "Завантаження перервалося.")?;
    let info = check_image(&header[..header_len], size, idle_slot_size()?).map_err(|e| e.to_string())?;

    let mut ota = EspOta::new().map_err(|e| format!("OTA недоступне: {e}"))?;
    let mut update = ota
        .initiate_update_with_known_size(size)
        .map_err(|e| format!("Не вдалося підготувати слот: {e}"))?;
    update.write(&header).map_err(|e| format!("Помилка запису: {e}"))?;
    let mut chunk = vec![0u8; 4096];
    let mut written = HEADER_LEN;
    while written < size {
        let len = chunk.len().min(size - written);
        body.read_exact(&mut chunk[..len]).map_err(|_| "Завантаження перервалося.")?;
        update.write(&chunk[..len]).map_err(|e| format!("Помилка запису: {e}"))?;
        written += len;
    }
    // Verifies the image checksum, then switches the boot slot.
    update.complete().map_err(|e| format!("Прошивка пошкоджена: {e}"))?;
    log::info!("ota: {} written, restarting", info.version);
    Ok(info)
}

fn idle_slot_size() -> Result<usize, String> {
    // SAFETY: a null argument asks ESP-IDF for the slot after the running one;
    // the returned partition entry is static.
    let partition = unsafe { esp_ota_get_next_update_partition(std::ptr::null()) };
    // SAFETY: non-null pointers from ESP-IDF point at the static partition table.
    unsafe { partition.as_ref() }
        .map(|p| p.size as usize)
        .ok_or_else(|| "Немає другого OTA-слота — прошийте пристрій через USB з partitions.csv.".to_string())
}
