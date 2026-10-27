//! SGP40 VOC raw signal with humidity and temperature compensation, per the
//! Sensirion datasheet.

use std::time::Duration;

use crate::{Bus, DriverError};

pub(crate) const ADDR: u8 = 0x59;
/// sgp40_execute_self_test, sent as a register write of its second byte.
const EXECUTE_SELF_TEST: [u8; 2] = [0x28, 0x0E];
const SELF_TEST_WAIT: Duration = Duration::from_millis(320);
const SELF_TEST_PASSED: u16 = 0xD400;
/// sgp40_measure_raw_signal is 0x260F: 0x26 goes out as the register byte,
/// 0x0F leads the arguments.
const MEASURE_RAW: u8 = 0x26;
const MEASURE_RAW_ARG: u8 = 0x0F;
const MEASURE_WAIT: Duration = Duration::from_millis(250);
const CRC_MISMATCH: DriverError = DriverError::Msg("SGP40 CRC mismatch");

pub(crate) struct Sgp40;

/// Sensirion CRC-8: polynomial 0x31, init 0xFF.
fn crc8(data: &[u8]) -> u8 {
    let mut crc = 0xFFu8;
    for &b in data {
        crc ^= b;
        for _ in 0..8 {
            crc = if crc & 0x80 != 0 { (crc << 1) ^ 0x31 } else { crc << 1 };
        }
    }
    crc
}

/// Arguments of measure_raw after its first byte: humidity ticks, CRC,
/// temperature ticks, CRC.
fn measure_args(rh_pct: f64, temp_c: f64) -> [u8; 7] {
    let ticks = |x: f64| (x.round().clamp(0.0, 65535.0) as u16).to_be_bytes();
    let rh = ticks(rh_pct * 65535.0 / 100.0);
    let t = ticks((temp_c + 45.0) * 65535.0 / 175.0);
    [MEASURE_RAW_ARG, rh[0], rh[1], crc8(&rh), t[0], t[1], crc8(&t)]
}

/// The response word, after checking its CRC.
fn response(bus: &mut dyn Bus) -> Result<u16, DriverError> {
    let mut b = [0; 3];
    bus.read_block(ADDR, 0, &mut b)?;
    if crc8(&b[..2]) != b[2] {
        return Err(CRC_MISMATCH);
    }
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

impl Sgp40 {
    pub(crate) fn new(bus: &mut dyn Bus) -> Result<Sgp40, DriverError> {
        bus.write_byte(ADDR, EXECUTE_SELF_TEST[0], EXECUTE_SELF_TEST[1])?;
        bus.sleep(SELF_TEST_WAIT);
        if response(bus)? != SELF_TEST_PASSED {
            return Err(DriverError::Msg("SGP40 self test failed"));
        }
        Ok(Sgp40)
    }

    /// Raw signal compensated for the given air humidity and temperature.
    pub(crate) fn measure(&self, bus: &mut dyn Bus, rh_pct: f64, temp_c: f64) -> Result<u16, DriverError> {
        bus.write_block(ADDR, MEASURE_RAW, &measure_args(rh_pct, temp_c))?;
        bus.sleep(MEASURE_WAIT);
        response(bus)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;

    #[test]
    fn crc_matches_the_datasheet_example() {
        assert_eq!(crc8(&[0xBE, 0xEF]), 0x92);
    }

    #[test]
    fn default_compensation_is_the_datasheet_default() {
        // Datasheet: 0x8000 / 0xA2 for 50 %RH and 0x6666 / 0x93 for 25 °C.
        assert_eq!(measure_args(50.0, 25.0), [0x0F, 0x80, 0x00, 0xA2, 0x66, 0x66, 0x93]);
    }

    #[test]
    fn compensation_ticks_clamp() {
        assert_eq!(measure_args(0.0, -45.0)[1..3], [0x00, 0x00]);
        assert_eq!(measure_args(120.0, 200.0)[1..3], [0xFF, 0xFF]);
        assert_eq!(measure_args(120.0, 200.0)[4..6], [0xFF, 0xFF]);
    }

    #[test]
    fn compensation_ticks_for_dry_hot_air() {
        assert_eq!(measure_args(9.87, 51.75), [0x0F, 0x19, 0x44, 0xD5, 0x8D, 0x87, 0x0F]);
    }

    fn sgp(measurement: &[u8]) -> Model {
        Model::default().block(ADDR, 0x00, &[&[0xD4, 0x00, 0xC6], measurement])
    }

    #[test]
    fn runs_only_the_documented_self_test() {
        let mut bus = sgp(&[0x7B, 0x63, 0x48]);
        Sgp40::new(&mut bus).unwrap();
        assert_eq!(bus.ops, ["wbd 59 28 0e", "sleep 0.32", "rbk 59 00 3"]);
    }

    #[test]
    fn a_failed_self_test_says_so() {
        // 0x4B00 with its CRC: a test that ran and failed.
        let mut bus = Model::default().block(ADDR, 0x00, &[&[0x4B, 0x00, crc8(&[0x4B, 0x00])]]);
        assert_eq!(Sgp40::new(&mut bus).err(), Some(DriverError::Msg("SGP40 self test failed")));
    }

    #[test]
    fn measure_checks_the_crc() {
        let mut bus = sgp(&[0x7B, 0x63, 0x48]);
        let sgp40 = Sgp40::new(&mut bus).unwrap();
        assert_eq!(sgp40.measure(&mut bus, 50.0, 25.0), Ok(31587));
        let mut bus = sgp(&[0x7B, 0x63, 0x00]);
        let sgp40 = Sgp40::new(&mut bus).unwrap();
        assert_eq!(sgp40.measure(&mut bus, 50.0, 25.0), Err(CRC_MISMATCH));
    }
}
