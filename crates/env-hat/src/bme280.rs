//! BME280 temperature, pressure and humidity, per the Bosch datasheet
//! (BST-BME280-DS002).

use std::time::Duration;

use crate::{Bus, DriverError};

pub(crate) const ADDR: u8 = 0x76;
const CALIB_00: u8 = 0x88;
const CALIB_26: u8 = 0xE1;
const CTRL_HUM: u8 = 0xF2;
const CTRL_MEAS: u8 = 0xF4;
const CONFIG: u8 = 0xF5;
const PRESS_MSB: u8 = 0xF7;
/// osrs_h x1.
const HUMIDITY_X1: u8 = 1;
/// osrs_t x2, osrs_p x4, normal mode.
const MEAS_T2_P4_NORMAL: u8 = (2 << 5) | (3 << 2) | 3;
/// 1000 ms standby, IIR filter coefficient 4.
const STANDBY_1000MS_FILTER_4: u8 = (5 << 5) | (2 << 2);
/// The longest conversion at these settings, datasheet 9.1:
/// 1.25 + 2.3·2 + (2.3·4 + 0.575) + (2.3·1 + 0.575) = 18.5 ms.
const FIRST_CONVERSION: Duration = Duration::from_millis(20);

/// Calibration coefficients dig_T1..T3, dig_P1..P9, dig_H1..H6.
#[derive(Debug, PartialEq)]
struct Calib {
    t: [i64; 3],
    p: [i64; 9],
    h: [i64; 6],
}

/// One compensated measurement at the board.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Sample {
    pub pressure_hpa: f64,
    pub temperature_c: f64,
    pub humidity_pct: f64,
}

pub(crate) struct Bme280 {
    calib: Calib,
    /// Carries the temperature into the pressure and humidity formulas.
    t_fine: f64,
}

/// Datasheet table 16 types: T1, P1 u16; T2, T3, P2..P9, H2 i16; H1, H3 u8;
/// H4, H5 signed 12-bit; H6 i8. `lo` is 0x88..0xA1, `hi` 0xE1..0xE7.
fn calib(lo: &[u8; 26], hi: &[u8; 7]) -> Calib {
    let u16_at = |i: usize| i64::from(u16::from_le_bytes([lo[i], lo[i + 1]]));
    let i16_at = |i: usize| i64::from(i16::from_le_bytes([lo[i], lo[i + 1]]));
    let s8 = |b: u8| i64::from(b as i8);
    Calib {
        t: [u16_at(0), i16_at(2), i16_at(4)],
        p: std::array::from_fn(|k| if k == 0 { u16_at(6) } else { i16_at(6 + 2 * k) }),
        h: [
            lo[25].into(),
            i16::from_le_bytes([hi[0], hi[1]]).into(),
            hi[2].into(),
            (s8(hi[3]) << 4) | i64::from(hi[4] & 0x0F),
            (s8(hi[5]) << 4) | i64::from(hi[4] >> 4),
            s8(hi[6]),
        ],
    }
}

impl Bme280 {
    pub(crate) fn new(bus: &mut dyn Bus) -> Result<Bme280, DriverError> {
        // Datasheet 5.4.6: config writes are only reliable in sleep mode, and
        // ctrl_hum takes effect at the next ctrl_meas write.
        bus.write_byte(ADDR, CTRL_MEAS, 0)?;
        bus.write_byte(ADDR, CTRL_HUM, HUMIDITY_X1)?;
        bus.write_byte(ADDR, CONFIG, STANDBY_1000MS_FILTER_4)?;
        bus.write_byte(ADDR, CTRL_MEAS, MEAS_T2_P4_NORMAL)?;
        if bus.read_byte(ADDR, CONFIG)? != STANDBY_1000MS_FILTER_4 {
            return Err(DriverError::Msg("BME280 ignored its config write"));
        }
        let mut lo = [0; 26];
        bus.read_block(ADDR, CALIB_00, &mut lo)?;
        let mut hi = [0; 7];
        bus.read_block(ADDR, CALIB_26, &mut hi)?;
        // Until it ends, the data registers hold their reset values.
        bus.sleep(FIRST_CONVERSION);
        Ok(Bme280 { calib: calib(&lo, &hi), t_fine: 0.0 })
    }

    /// One burst read of the data registers, temperature compensated first
    /// so `t_fine` is current for pressure and humidity.
    pub(crate) fn read(&mut self, bus: &mut dyn Bus) -> Result<Sample, DriverError> {
        let mut d = [0; 8];
        bus.read_block(ADDR, PRESS_MSB, &mut d)?;
        let d = d.map(i64::from);
        let adc_p = (d[0] << 12) | (d[1] << 4) | (d[2] >> 4);
        let adc_t = (d[3] << 12) | (d[4] << 4) | (d[5] >> 4);
        let temperature_c = self.compensate_t(adc_t);
        let pressure_hpa = self.compensate_p(adc_p).ok_or(DriverError::Msg("BME280 pressure calibration is zero"))?;
        let humidity_pct = self.compensate_h((d[6] << 8) | d[7]);
        Ok(Sample { pressure_hpa, temperature_c, humidity_pct })
    }

    /// Datasheet 8.1, double precision. `None` where the reference returns 0
    /// to avoid dividing by zero.
    fn compensate_p(&self, adc_p: i64) -> Option<f64> {
        let [p1, p2, p3, p4, p5, p6, p7, p8, p9] = self.calib.p.map(|v| v as f64);
        let mut v1 = (self.t_fine / 2.0) - 64000.0;
        let mut v2 = v1 * v1 * p6 / 32768.0;
        v2 += v1 * p5 * 2.0;
        v2 = (v2 / 4.0) + (p4 * 65536.0);
        v1 = (p3 * v1 * v1 / 524288.0 + p2 * v1) / 524288.0;
        v1 = (1.0 + v1 / 32768.0) * p1;
        if v1 == 0.0 {
            return None;
        }
        let mut p = 1048576.0 - adc_p as f64;
        p = (p - (v2 / 4096.0)) * 6250.0 / v1;
        v1 = p9 * p * p / 2147483648.0;
        v2 = p * p8 / 32768.0;
        Some((p + (v1 + v2 + p7) / 16.0) / 100.0)
    }

    fn compensate_t(&mut self, adc_t: i64) -> f64 {
        let adc_t = adc_t as f64;
        let [t1, t2, t3] = self.calib.t.map(|v| v as f64);
        let v1 = (adc_t / 16384.0 - t1 / 1024.0) * t2;
        let v2 = (adc_t / 131072.0 - t1 / 8192.0) * (adc_t / 131072.0 - t1 / 8192.0) * t3;
        self.t_fine = v1 + v2;
        self.t_fine / 5120.0
    }

    /// Datasheet 8.1, double precision, clamped to 0..=100 %.
    fn compensate_h(&self, adc_h: i64) -> f64 {
        let [h1, h2, h3, h4, h5, h6] = self.calib.h.map(|v| v as f64);
        let var_h = self.t_fine - 76800.0;
        let var_h = (adc_h as f64 - (h4 * 64.0 + h5 / 16384.0 * var_h))
            * (h2 / 65536.0 * (1.0 + h6 / 67108864.0 * var_h * (1.0 + h3 / 67108864.0 * var_h)));
        (var_h * (1.0 - h1 * var_h / 524288.0)).clamp(0.0, 100.0)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::model::{Model, round};

    #[test]
    fn calibration_signs_every_signed_coefficient() {
        // Live-chip bytes with T3, P9, H4, H5 and H6 replaced by negative values.
        let lo = [
            0x95, 0x6F, 0xE5, 0x68, 0xCE, 0xFF, 0xA4, 0x8F, 0xE3, 0xD6, 0xD0, 0x0B, 0xCC, 0x1B, 0xFB, 0xFF, //
            0xF9, 0xFF, 0xAC, 0x26, 0x0A, 0xD8, 0x43, 0xEF, 0x00, 0x4B,
        ];
        let hi = [0x7D, 0x01, 0x00, 0xEE, 0x93, 0xFD, 0xE2];
        assert_eq!(
            calib(&lo, &hi),
            Calib { t: [28565, 26853, -50], p: [36772, -10525, 3024, 7116, -5, -7, 9900, -10230, -4285], h: [75, 381, 0, -285, -39, -30] }
        );
    }

    /// The live chip's calibration (0x88..0x9F, 0xA1, 0xE1..0xE7) and its first
    /// data registers (0xF7..0xFE) recorded from a live board.
    pub(crate) fn live(model: Model) -> Model {
        let c = [
            0x95, 0x6f, 0xe5, 0x68, 0x32, 0x00, 0xa4, 0x8f, 0xe3, 0xd6, 0xd0, 0x0b, 0xcc, 0x1b, 0xfb, 0xff, //
            0xf9, 0xff, 0xac, 0x26, 0x0a, 0xd8, 0xbd, 0x10, 0x4b, 0x7d, 0x01, 0x00, 0x11, 0x24, 0x03, 0x1e,
        ];
        model.reg(ADDR, 0x88, &c[..24]).reg(ADDR, 0xA1, &c[24..25]).reg(ADDR, 0xE1, &c[25..]).reg(
            ADDR,
            0xF7,
            &[0x59, 0xf3, 0x00, 0x97, 0x0a, 0x00, 0x4d, 0x5e],
        )
    }

    #[test]
    fn configures_in_sleep_mode_and_waits_for_the_first_conversion() {
        let mut bus = live(Model::default());
        let mut bme = Bme280::new(&mut bus).unwrap();
        assert_eq!(
            bus.ops,
            ["wbd 76 f4 00", "wbd 76 f2 01", "wbd 76 f5 a8", "wbd 76 f4 4f", "rbd 76 f5", "rbk 76 88 26", "rbk 76 e1 7", "sleep 0.02",]
        );
        let s = bme.read(&mut bus).unwrap();
        assert_eq!([s.pressure_hpa, s.temperature_c, s.humidity_pct].map(|x| round(x, 2)), [1002.34, 51.75, 9.87]);
        assert_eq!(bus.ops[8..], ["rbk 76 f7 8"]);
    }

    struct IgnoresConfig(Model);

    impl Bus for IgnoresConfig {
        fn read_byte(&mut self, addr: u8, reg: u8) -> Result<u8, crate::BusError> {
            self.0.read_byte(addr, reg)
        }
        fn write_byte(&mut self, addr: u8, reg: u8, value: u8) -> Result<(), crate::BusError> {
            if reg == CONFIG { Ok(()) } else { self.0.write_byte(addr, reg, value) }
        }
        fn read_block(&mut self, addr: u8, reg: u8, buf: &mut [u8]) -> Result<(), crate::BusError> {
            self.0.read_block(addr, reg, buf)
        }
        fn write_block(&mut self, addr: u8, reg: u8, data: &[u8]) -> Result<(), crate::BusError> {
            self.0.write_block(addr, reg, data)
        }
    }

    #[test]
    fn refuses_a_chip_that_ignored_its_config() {
        let mut bus = IgnoresConfig(live(Model::default()));
        assert_eq!(Bme280::new(&mut bus).err(), Some(DriverError::Msg("BME280 ignored its config write")));
    }

    /// The worked example in the BMP280 datasheet (rev 1.19, section 8.2),
    /// whose temperature and pressure compensation the BME280 shares.
    #[test]
    fn compensation_matches_the_bosch_example() {
        let mut bme = Bme280 {
            calib: Calib { t: [27504, 26435, -1000], p: [36477, -10685, 3024, 2855, 140, -7, 15500, -14600, 6000], h: [0; 6] },
            t_fine: 0.0,
        };
        assert_eq!(round(bme.compensate_t(519888), 2), 25.08);
        assert_eq!(bme.t_fine.round(), 128422.0);
        assert_eq!(round(bme.compensate_p(415148).unwrap() * 100.0, 2), 100653.27);
    }
}
