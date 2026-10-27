//! LTR390 UV and ambient light, per the Lite-On datasheet (LTR-390UV-01).

use std::time::Duration;

use crate::hat::Chip;
use crate::reading::{Reading, Uv};
use crate::{Bus, DriverError};

pub(crate) const ADDR: u8 = 0x53;
const MAIN_CTRL: u8 = 0x00;
const MEAS_RATE: u8 = 0x04;
const GAIN: u8 = 0x05;
const PART_ID: u8 = 0x06;
const MAIN_STATUS: u8 = 0x07;
const ALS_DATA_0: u8 = 0x0D;
const UVS_DATA_0: u8 = 0x10;
const DATA_STATUS: u8 = 0x08;
const RESOLUTION_20BIT: u8 = 0x00;
const RATE_500MS: u8 = 0x4;
const GAIN_3: u8 = 0x1;
const GAIN_18: u8 = 0x4;
/// UV counts per UV index at gain 18 and 20-bit resolution, no window:
/// LTR-390UV-01 Final DS V1.2, table 4.5 and figure 4.4
/// (https://files.waveshare.com/upload/a/ac/1_LTR-390UV_Final_DS_V1_2.pdf).
const UV_SENSITIVITY: f64 = 1400.0;
const POLL: Duration = Duration::from_millis(100);
/// 15 polls: three conversion periods at 500 ms.
const POLLS: usize = 15;
const UV_TIMEOUT: DriverError = DriverError::Msg("UV conversion timed out");

pub(crate) struct Ltr390 {
    /// False while the chip may still be in ALS mode: its UVS data would
    /// then be stale.
    in_uvs_mode: bool,
}

fn uv_index(uvs: u32) -> f64 {
    f64::from(uvs) / UV_SENSITIVITY
}

/// Lux from ALS counts: datasheet `0.6 * ALS / (gain * int)`, with int = 4
/// at 20-bit / 400 ms and gain 3, the gain range its lux accuracy is
/// specified for.
fn als_lux(als: u32) -> f64 {
    0.6 * f64::from(als) / (3.0 * 4.0)
}

/// What the chip measures. Each mode has its own gain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Uv,
    AmbientLight,
}

impl Mode {
    /// MAIN_CTRL: the mode bit and ALS_UVS_Enable.
    fn main_ctrl(self) -> u8 {
        match self {
            Mode::Uv => 0x0A,
            Mode::AmbientLight => 0x02,
        }
    }

    /// UV at gain 18, where the datasheet gives the UV sensitivity. Ambient
    /// light at gain 3, the gain its lux accuracy is specified for.
    fn gain(self) -> u8 {
        match self {
            Mode::Uv => GAIN_18,
            Mode::AmbientLight => GAIN_3,
        }
    }
}

fn enter(bus: &mut dyn Bus, mode: Mode) -> Result<(), DriverError> {
    bus.write_byte(ADDR, GAIN, mode.gain())?;
    Ok(bus.write_byte(ADDR, MAIN_CTRL, mode.main_ctrl())?)
}

/// Switch to `mode` and wait up to 1.5 s for a fresh conversion; false on
/// timeout.
fn switch_and_wait(bus: &mut dyn Bus, mode: Mode) -> Result<bool, DriverError> {
    enter(bus, mode)?;
    bus.read_byte(ADDR, MAIN_STATUS)?; // reading clears a stale data-ready flag
    for _ in 0..POLLS {
        bus.sleep(POLL);
        if bus.read_byte(ADDR, MAIN_STATUS)? & DATA_STATUS != 0 {
            return Ok(true);
        }
    }
    Ok(false)
}

/// A 20-bit result from three data registers, least significant first.
fn read_20bit(bus: &mut dyn Bus, first: u8) -> Result<u32, DriverError> {
    let mut b = [0; 3];
    bus.read_block(ADDR, first, &mut b)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2] & 0x0F, 0]))
}

/// Switch to ALS mode and read one conversion. Leaves the chip in ALS mode;
/// the caller restores UVS mode.
fn als_once(bus: &mut dyn Bus) -> Result<Option<u32>, DriverError> {
    if !switch_and_wait(bus, Mode::AmbientLight)? {
        return Ok(None);
    }
    read_20bit(bus, ALS_DATA_0).map(Some)
}

impl Ltr390 {
    pub(crate) fn new(bus: &mut dyn Bus) -> Result<Ltr390, DriverError> {
        // The low nibble is the silicon revision.
        if bus.read_byte(ADDR, PART_ID)? >> 4 != 0xB {
            return Err(DriverError::Msg("LTR390 part ID mismatch"));
        }
        bus.write_byte(ADDR, MEAS_RATE, RESOLUTION_20BIT | RATE_500MS)?;
        // Not measuring yet, so the first read waits for a UV conversion.
        Ok(Ltr390 { in_uvs_mode: false })
    }

    /// The UV conversion, then one ambient-light conversion, leaving the
    /// chip measuring UV again.
    pub(crate) fn read(&mut self, bus: &mut dyn Bus) -> Result<Uv, DriverError> {
        if !self.in_uvs_mode && !switch_and_wait(bus, Mode::Uv)? {
            return Err(UV_TIMEOUT);
        }
        let counts = read_20bit(bus, UVS_DATA_0)?;
        self.in_uvs_mode = false;
        let als = als_once(bus);
        enter(bus, Mode::Uv)?;
        self.in_uvs_mode = true;
        Ok(Uv { index: uv_index(counts), counts, ambient_lux: als?.map(als_lux) })
    }
}

impl Chip for Ltr390 {
    type Group = Uv;
    const NAME: &'static str = "ltr390";
    const PERIOD: Duration = Duration::from_secs(10);

    fn sample(&mut self, bus: &mut dyn Bus) -> Result<Uv, DriverError> {
        self.read(bus)
    }

    fn publish(&self, reading: &mut Reading, uv: Option<Uv>) {
        reading.uv = uv;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::BusError;
    use crate::model::Model;

    fn ltr(status: u8) -> Model {
        Model::default().reg(ADDR, 0x06, &[0xB2, status]).reg(ADDR, 0x10, &[0x10, 0x27, 0x00]).reg(ADDR, 0x0D, &[0x40, 0x9C, 0x00])
    }

    /// Pins the datasheet promise: 1400 counts per UVI at gain 18, 20-bit
    /// (LTR-390UV-01 Final DS V1.2, table 4.5). V1.1 said 2300.
    #[test]
    fn one_uv_index_is_1400_counts() {
        assert_eq!(uv_index(1400), 1.0);
    }

    #[test]
    fn counts_are_20_bits() {
        let mut bus = Model::default().reg(ADDR, 0x10, &[0xFF, 0xFF, 0xFF]);
        assert_eq!(read_20bit(&mut bus, UVS_DATA_0), Ok(0xF_FFFF));
    }

    #[test]
    fn rejects_a_wrong_part_id_without_writing() {
        let mut bus = Model::default().reg(ADDR, 0x06, &[0xA2]);
        assert_eq!(Ltr390::new(&mut bus).err(), Some(DriverError::Msg("LTR390 part ID mismatch")));
        assert_eq!(bus.ops, ["rbd 53 06"]);
    }

    #[test]
    fn accepts_any_silicon_revision() {
        let mut bus = Model::default().reg(ADDR, 0x06, &[0xB3]);
        Ltr390::new(&mut bus).unwrap();
        assert_eq!(bus.ops, ["rbd 53 06", "wbd 53 04 04"]);
    }

    /// One read with the chip not yet measuring UV: enter UVS mode at gain
    /// 18 and wait for a conversion, read it, then the same for ALS at gain
    /// 3, then back to UVS.
    const READ_FROM_STANDBY: [&str; 14] = [
        "wbd 53 05 04",
        "wbd 53 00 0a",
        "rbd 53 07",
        "sleep 0.1",
        "rbd 53 07",
        "rbk 53 10 3",
        "wbd 53 05 01",
        "wbd 53 00 02",
        "rbd 53 07",
        "sleep 0.1",
        "rbd 53 07",
        "rbk 53 0d 3",
        "wbd 53 05 04",
        "wbd 53 00 0a",
    ];

    #[test]
    fn the_first_read_waits_for_a_uv_conversion() {
        let mut bus = ltr(0x08);
        let mut s = Ltr390::new(&mut bus).unwrap();
        assert_eq!(bus.ops, ["rbd 53 06", "wbd 53 04 04"]);
        assert_eq!(s.read(&mut bus), Ok(Uv { index: 7.142857142857143, counts: 10000, ambient_lux: Some(2000.0) }));
        assert_eq!(bus.ops[2..], READ_FROM_STANDBY);
    }

    #[test]
    fn later_reads_take_the_running_uv_conversion() {
        let mut bus = ltr(0x08);
        let mut s = Ltr390::new(&mut bus).unwrap();
        s.read(&mut bus).unwrap();
        let before = bus.ops.len();
        s.read(&mut bus).unwrap();
        assert_eq!(bus.ops[before..], READ_FROM_STANDBY[5..]);
    }

    #[test]
    fn an_als_timeout_still_restores_uv_mode() {
        // MAIN_STATUS: the UV wait's clear and ready, then never ready.
        let mut bus = ltr(0x00).bytes(ADDR, MAIN_STATUS, &[0x00, 0x08, 0x00]);
        let mut s = Ltr390::new(&mut bus).unwrap();
        assert_eq!(s.read(&mut bus).map(|uv| (uv.counts, uv.ambient_lux)), Ok((10000, None)));
        assert_eq!(bus.ops.iter().filter(|o| *o == "sleep 0.1").count(), 1 + 15);
        assert_eq!(bus.ops[bus.ops.len() - 2..], ["wbd 53 05 04", "wbd 53 00 0a"]);
    }

    #[test]
    fn a_failed_uv_restore_reasserts_uv_mode_before_the_next_read() {
        let mut bus = ltr(0x08).block(ADDR, 0x10, &[&[0x10, 0x27, 0x00], &[0x20, 0x4E, 0x00]]);
        bus.nack = Some(("wbd 53 00 0a", 2)); // read 1 enters UVS mode, then fails to restore it
        let mut s = Ltr390::new(&mut bus).unwrap();
        assert_eq!(s.read(&mut bus), Err(DriverError::Bus(BusError(5))));
        let before = bus.ops.len();
        assert_eq!(s.read(&mut bus).map(|uv| uv.counts), Ok(20000));
        assert_eq!(bus.ops[before..], READ_FROM_STANDBY);
    }

    #[test]
    fn a_uv_timeout_fails_rather_than_report_stale_data() {
        let mut bus = ltr(0x00);
        let mut s = Ltr390::new(&mut bus).unwrap();
        assert_eq!(s.read(&mut bus), Err(UV_TIMEOUT));
        assert!(!bus.ops.contains(&"rbk 53 10 3".to_string()), "{:?}", bus.ops);
    }
}
