//! JK BMS over Bluetooth LE, read-only (protocol "JK02", byte layout as
//! decoded by esphome-jk-bms). The BMS answers on one characteristic with
//! 300-byte frames split into notifications; a frame is
//! `55 AA EB 90 <type> <counter> <payload…> <sum of the first 299 bytes>`.

use serde::Serialize;

/// GATT service and characteristic the BMS talks through.
pub const SERVICE: u16 = 0xFFE0;
pub const CHARACTERISTIC: u16 = 0xFFE1;

const FRAME_LEN: usize = 300;
const HEADER: [u8; 4] = [0x55, 0xAA, 0xEB, 0x90];
/// Longer than any frame: something got lost, start over.
const MAX_BUFFER: usize = 400;

/// What to ask the BMS for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request {
    /// Model and versions; answered once with a device info frame.
    DeviceInfo,
    /// Measurements; the BMS then keeps sending cell info frames about once a second.
    CellInfo,
}

/// The 20-byte command for `request`.
pub fn command(request: Request) -> [u8; 20] {
    let mut frame = [0; 20];
    frame[..4].copy_from_slice(&[0xAA, 0x55, 0x90, 0xEB]);
    frame[4] = match request {
        Request::DeviceInfo => 0x97,
        Request::CellInfo => 0x96,
    };
    frame[19] = sum(&frame[..19]);
    frame
}

fn sum(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |acc, &b| acc.wrapping_add(b))
}

/// Glues notifications into whole frames.
#[derive(Debug, Default)]
pub struct Assembler {
    buffer: Vec<u8>,
}

/// A frame whose checksum matched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    DeviceInfo(DeviceInfo),
    /// Measurements; how to read them depends on the device's layout.
    CellInfo(Vec<u8>),
    Other(u8),
}

impl Assembler {
    /// Adds one notification; returns the frame it completes, if any.
    pub fn push(&mut self, chunk: &[u8]) -> Option<Frame> {
        if chunk.starts_with(&HEADER) || self.buffer.len() + chunk.len() > MAX_BUFFER {
            self.buffer.clear();
        }
        self.buffer.extend_from_slice(chunk);
        if self.buffer.len() < FRAME_LEN {
            return None;
        }
        // Some units send 320 bytes; the checksum still closes byte 300.
        let frame: Vec<u8> = self.buffer.drain(..).take(FRAME_LEN).collect();
        if !frame.starts_with(&HEADER) || sum(&frame[..FRAME_LEN - 1]) != frame[FRAME_LEN - 1] {
            return None;
        }
        Some(match frame[4] {
            0x02 => Frame::CellInfo(frame),
            0x03 => Frame::DeviceInfo(DeviceInfo {
                model: text(&frame[6..22]),
                hardware: text(&frame[22..30]),
                software: text(&frame[30..38]),
            }),
            kind => Frame::Other(kind),
        })
    }
}

/// Model and versions from the device info frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeviceInfo {
    pub model: String,
    pub hardware: String,
    pub software: String,
}

/// Where the cell info fields sit: hardware 11 and later report up to 32
/// cells, which moves everything after the cell list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Cells24,
    Cells32,
}

impl DeviceInfo {
    pub fn layout(&self) -> Layout {
        let major: String = self.hardware.chars().take_while(char::is_ascii_digit).collect();
        match major.parse::<u32>() {
            Ok(major) if major >= 11 => Layout::Cells32,
            _ => Layout::Cells24,
        }
    }
}

/// One set of measurements, in the units the BMS sends.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reading {
    pub voltage_mv: u32,
    /// Positive while charging.
    pub current_ma: i32,
    pub soc: u8,
    pub remaining_mah: u32,
    pub capacity_mah: u32,
    pub cycles: u32,
    /// Connected cells in order.
    pub cells_mv: Vec<u16>,
    /// Tenths of a degree Celsius.
    pub temp1_dc: i16,
    pub temp2_dc: i16,
    pub mosfet_dc: i16,
    pub charging: bool,
    pub discharging: bool,
}

/// Reads a cell info frame laid out as `layout`.
pub fn reading(frame: &[u8], layout: Layout) -> Reading {
    let (cells, shift) = match layout {
        Layout::Cells24 => (24, 0),
        Layout::Cells32 => (32, 32),
    };
    let u16_at = |i: usize| u16::from_le_bytes([frame[i], frame[i + 1]]);
    let i16_at = |i: usize| i16::from_le_bytes([frame[i], frame[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]);
    // The fields past the cell list move by `shift`.
    let at = |i: usize| i + shift;
    Reading {
        voltage_mv: u32_at(at(118)),
        current_ma: u32_at(at(126)) as i32,
        soc: frame[at(141)],
        remaining_mah: u32_at(at(142)),
        capacity_mah: u32_at(at(146)),
        cycles: u32_at(at(150)),
        cells_mv: (0..cells).map(|n| u16_at(6 + 2 * n)).filter(|&mv| mv > 0).collect(),
        temp1_dc: i16_at(at(130)),
        temp2_dc: i16_at(at(132)),
        // The 32-cell layout reports errors where the older one had this.
        mosfet_dc: i16_at(match layout {
            Layout::Cells24 => 134,
            Layout::Cells32 => at(112),
        }),
        charging: frame[at(166)] != 0,
        discharging: frame[at(167)] != 0,
    }
}

/// A zero-padded ASCII field.
fn text(bytes: &[u8]) -> String {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jk_frames::*;

    /// Feeds `frame` the way BLE delivers it: 20-byte notifications.
    fn receive(frame: &[u8]) -> Option<Frame> {
        let mut assembler = Assembler::default();
        let mut result = None;
        for chunk in frame.chunks(20) {
            result = assembler.push(chunk).or(result);
        }
        result
    }

    fn device(frame: &[u8]) -> DeviceInfo {
        match receive(frame) {
            Some(Frame::DeviceInfo(info)) => info,
            other => panic!("not a device info frame: {other:?}"),
        }
    }

    fn measured(device_frame: &[u8], cells_frame: &[u8]) -> Reading {
        let layout = device(device_frame).layout();
        match receive(cells_frame) {
            Some(Frame::CellInfo(bytes)) => reading(&bytes, layout),
            other => panic!("not a cell info frame: {other:?}"),
        }
    }

    #[test]
    fn commands_are_framed_and_summed() {
        let device = command(Request::DeviceInfo);
        assert_eq!(device[..6], [0xAA, 0x55, 0x90, 0xEB, 0x97, 0x00]);
        assert!(device[6..19].iter().all(|&b| b == 0));
        assert_eq!(device[19], 0x11); // (0xAA + 0x55 + 0x90 + 0xEB + 0x97) mod 256
        assert_eq!(command(Request::CellInfo)[4], 0x96);
        assert_eq!(command(Request::CellInfo)[19], 0x10);
    }

    #[test]
    fn device_info_names_model_and_versions() {
        assert_eq!(
            device(&DEVICE_INFO_JK02_24S_V10),
            DeviceInfo { model: "JK-B2A24S20P".into(), hardware: "10.XG".into(), software: "10.07".into() }
        );
        assert_eq!(device(&DEVICE_INFO_JK02_32S_V15).hardware, "15A");
    }

    #[test]
    fn hardware_from_11_on_uses_the_32_cell_layout() {
        assert_eq!(device(&DEVICE_INFO_JK02_24S_V10).layout(), Layout::Cells24);
        assert_eq!(device(&DEVICE_INFO_JK02_32S_V11).layout(), Layout::Cells32);
        assert_eq!(device(&DEVICE_INFO_JK02_32S_V15).layout(), Layout::Cells32);
    }

    #[test]
    fn reads_a_24_cell_unit() {
        let r = measured(&DEVICE_INFO_JK02_24S_V10, &CELL_INFO_JK02_24S_V10);
        assert_eq!(r.cells_mv.len(), 24);
        assert_eq!((r.cells_mv[2], r.cells_mv[22]), (3279, 3285));
        assert_eq!((r.voltage_mv, r.current_ma, r.soc), (78_735, 0, 85));
        assert_eq!((r.remaining_mah, r.capacity_mah, r.cycles), (42_804, 50_000, 0));
        assert_eq!((r.temp1_dc, r.temp2_dc, r.mosfet_dc), (207, 207, 234));
        assert_eq!((r.charging, r.discharging), (true, false));
    }

    #[test]
    fn reads_temperatures_below_zero_and_unused_cells() {
        let r = measured(&DEVICE_INFO_JK02_24S_V10, &CELL_INFO_JK02_24S_V10_NEG_TEMPS);
        assert_eq!(r.cells_mv.len(), 13);
        assert_eq!((r.temp1_dc, r.temp2_dc, r.mosfet_dc), (-31, -29, 5));
        assert_eq!((r.soc, r.cycles, r.charging, r.discharging), (47, 56, false, false));
    }

    #[test]
    fn reads_a_32_cell_unit() {
        let r = measured(&DEVICE_INFO_JK02_32S_V11, &CELL_INFO_JK02_32S_V11);
        assert_eq!(r.cells_mv.len(), 16);
        assert_eq!((r.cells_mv[0], r.cells_mv[10], r.cells_mv[15]), (3246, 3216, 3242));
        assert_eq!((r.voltage_mv, r.current_ma, r.soc), (51_689, 0, 52));
        assert_eq!((r.remaining_mah, r.capacity_mah), (1_043, 2_000));
        assert_eq!((r.temp1_dc, r.temp2_dc, r.mosfet_dc), (177, 177, 173));
        assert_eq!((r.charging, r.discharging), (true, true));
    }

    #[test]
    fn reads_charging_current_and_cycles() {
        let r = measured(&DEVICE_INFO_JK02_32S_V15, &CELL_INFO_JK02_32S_V15);
        assert_eq!((r.voltage_mv, r.current_ma, r.soc, r.cycles), (53_224, 31_881, 25, 9));
        assert_eq!((r.remaining_mah, r.capacity_mah), (49_286, 200_000));
        assert_eq!((r.temp1_dc, r.temp2_dc, r.mosfet_dc), (134, 128, 129));
    }

    #[test]
    fn frames_with_a_wrong_checksum_are_dropped() {
        let mut frame = CELL_INFO_JK02_32S_V11;
        frame[100] ^= 1;
        assert_eq!(receive(&frame), None);
    }

    #[test]
    fn a_new_header_discards_a_broken_frame() {
        let mut assembler = Assembler::default();
        assert_eq!(assembler.push(&CELL_INFO_JK02_32S_V11[..120]), None);
        let mut result = None;
        for chunk in CELL_INFO_JK02_32S_V15.chunks(64) {
            result = assembler.push(chunk).or(result);
        }
        assert_eq!(result, Some(Frame::CellInfo(CELL_INFO_JK02_32S_V15.to_vec())));
    }

    #[test]
    fn noise_without_a_header_never_grows_without_bound() {
        let mut assembler = Assembler::default();
        for _ in 0..100 {
            assert_eq!(assembler.push(&[0x11; 20]), None);
        }
        assert!(assembler.buffer.len() <= MAX_BUFFER);
    }
}
