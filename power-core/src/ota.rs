//! Checks an uploaded firmware image before it is written to the OTA slot.
//! ESP-IDF verifies the checksum at the end; this rejects obviously wrong
//! files up front, before the slot is erased.

use std::fmt;

/// Image header (24 bytes), first segment header (8) and the app
/// description up to the build date: enough to identify the image.
pub const HEADER_LEN: usize = 144;

const IMAGE_MAGIC: u8 = 0xE9;
const CHIP_ESP32: u16 = 0x0000;
const APP_DESC_MAGIC: u32 = 0xABCD_5432;
const APP_DESC_OFFSET: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInfo {
    pub version: String,
    /// Build date and time, as ESP-IDF records them.
    pub built: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    NotAnImage,
    WrongChip,
    TooLarge { size: usize, max: usize },
}

impl fmt::Display for ImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImageError::NotAnImage => f.write_str(
                "Це не файл прошивки ESP32. Потрібен .bin, створений ./flash.sh --image.",
            ),
            ImageError::WrongChip => f.write_str("Прошивка зібрана для іншого чипа, не для ESP32."),
            ImageError::TooLarge { size, max } => {
                write!(f, "Прошивка завелика: {} КБ, а слот вміщує {} КБ.", size / 1024, max / 1024)
            }
        }
    }
}

/// Checks the first `HEADER_LEN` bytes of an image of `size` bytes that is
/// to go into a slot of `slot_size` bytes.
pub fn check_image(header: &[u8], size: usize, slot_size: usize) -> Result<ImageInfo, ImageError> {
    if header.len() < HEADER_LEN || header[0] != IMAGE_MAGIC {
        return Err(ImageError::NotAnImage);
    }
    let desc_magic = u32::from_le_bytes(header[APP_DESC_OFFSET..APP_DESC_OFFSET + 4].try_into().unwrap());
    if desc_magic != APP_DESC_MAGIC {
        return Err(ImageError::NotAnImage);
    }
    if u16::from_le_bytes([header[12], header[13]]) != CHIP_ESP32 {
        return Err(ImageError::WrongChip);
    }
    if size > slot_size {
        return Err(ImageError::TooLarge { size, max: slot_size });
    }
    // esp_app_desc_t: version at +16, time at +80, date at +96 (32/16/16 bytes).
    let text = |at: usize, len: usize| {
        let start = APP_DESC_OFFSET + at;
        let field = &header[start..start + len];
        let end = field.iter().position(|&b| b == 0).unwrap_or(len);
        String::from_utf8_lossy(&field[..end]).into_owned()
    };
    Ok(ImageInfo { version: text(16, 32), built: format!("{} {}", text(96, 16), text(80, 16)) })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLOT: usize = 0x1E_0000;

    /// A header shaped like the one espflash save-image writes.
    fn header(magic: u8, chip: u16, desc_magic: u32) -> Vec<u8> {
        let mut h = vec![0u8; HEADER_LEN];
        h[0] = magic;
        h[1] = 6;
        h[12..14].copy_from_slice(&chip.to_le_bytes());
        h[32..36].copy_from_slice(&desc_magic.to_le_bytes());
        h[48..61].copy_from_slice(b"2f50dc8-dirty");
        h[112..120].copy_from_slice(b"17:37:32");
        h[128..139].copy_from_slice(b"Oct 29 2026");
        h
    }

    fn valid() -> Vec<u8> {
        header(IMAGE_MAGIC, CHIP_ESP32, APP_DESC_MAGIC)
    }

    #[test]
    fn reads_version_and_build_date() {
        assert_eq!(
            check_image(&valid(), 1_357_440, SLOT),
            Ok(ImageInfo {
                version: "2f50dc8-dirty".to_string(),
                built: "Oct 29 2026 17:37:32".to_string(),
            })
        );
    }

    #[test]
    fn rejects_files_that_are_not_images() {
        assert_eq!(check_image(&valid()[..HEADER_LEN - 1], 1_000, SLOT), Err(ImageError::NotAnImage));
        assert_eq!(check_image(b"<html>", 6, SLOT), Err(ImageError::NotAnImage));
        let header = header(0x00, CHIP_ESP32, APP_DESC_MAGIC);
        assert_eq!(check_image(&header, 1_000, SLOT), Err(ImageError::NotAnImage));
    }

    #[test]
    fn rejects_bootloader_and_merged_images_without_an_app_description() {
        let header = header(IMAGE_MAGIC, CHIP_ESP32, 0);
        assert_eq!(check_image(&header, 1_000, SLOT), Err(ImageError::NotAnImage));
    }

    #[test]
    fn rejects_images_for_other_chips() {
        let esp32c3 = header(IMAGE_MAGIC, 0x0005, APP_DESC_MAGIC);
        assert_eq!(check_image(&esp32c3, 1_000, SLOT), Err(ImageError::WrongChip));
    }

    #[test]
    fn rejects_images_larger_than_the_slot() {
        assert_eq!(
            check_image(&valid(), SLOT + 1, SLOT),
            Err(ImageError::TooLarge { size: SLOT + 1, max: SLOT })
        );
        assert!(check_image(&valid(), SLOT, SLOT).is_ok());
    }

    #[test]
    fn explains_size_in_kilobytes() {
        let error = ImageError::TooLarge { size: 2_048_000, max: SLOT };
        assert_eq!(error.to_string(), "Прошивка завелика: 2000 КБ, а слот вміщує 1920 КБ.");
    }
}
