//! ICM20948 accelerometer and gyroscope with its AK09916 magnetometer, per
//! the TDK and AKM datasheets. The ICM's I2C master reads the magnetometer
//! continuously into EXT_SLV_SENS_DATA, so one burst read of bank 0 returns
//! every axis.

use std::time::Duration;

use crate::hat::{Chip, log};
use crate::orientation;
use crate::reading::{Motion, Reading};
use crate::{Bus, DriverError};

pub(crate) const ADDR: u8 = 0x68;
const AK09916: u8 = 0x0C;
const AUX_READ: u8 = 0x80;

const REG_BANK_SEL: u8 = 0x7F;
// Bank 0.
const WHO_AM_I: u8 = 0x00;
const USER_CTRL: u8 = 0x03;
const I2C_MST_EN: u8 = 0x20;
const I2C_MST_RST: u8 = 0x02;
const PWR_MGMT_1: u8 = 0x06;
const DEVICE_RESET: u8 = 0x80;
const CLKSEL_AUTO: u8 = 0x01;
const I2C_MST_STATUS: u8 = 0x17;
const I2C_SLV4_NACK: u8 = 0x10;
const I2C_SLV4_DONE: u8 = 0x40;
const ACCEL_XOUT_H: u8 = 0x2D;
// Bank 2.
const GYRO_SMPLRT_DIV: u8 = 0x00;
const GYRO_CONFIG_1: u8 = 0x01;
const ACCEL_SMPLRT_DIV_2: u8 = 0x11;
const ACCEL_CONFIG: u8 = 0x14;
// Bank 3.
const I2C_MST_CTRL: u8 = 0x01;
const I2C_SLV0_ADDR: u8 = 0x03;
const I2C_SLV0_REG: u8 = 0x04;
const I2C_SLV0_CTRL: u8 = 0x05;
const I2C_SLV4_ADDR: u8 = 0x13;
const I2C_SLV4_REG: u8 = 0x14;
const I2C_SLV4_CTRL: u8 = 0x15;
const I2C_SLV4_DO: u8 = 0x16;
const I2C_SLV4_DI: u8 = 0x17;
const I2C_SLV_EN: u8 = 0x80;
// AK09916.
const WIA1: u8 = 0x00;
const ST1: u8 = 0x10;
const ST2_HOFL: u8 = 0x08;
const CNTL2: u8 = 0x31;
const CONTINUOUS_100HZ: u8 = 0x08;
const CNTL3: u8 = 0x32;
const SOFT_RESET: u8 = 0x01;

/// Accel 6, gyro 6, temperature 2, then AK09916 ST1, HX..HZ, TMPS, ST2.
const BURST_LEN: usize = 23;
const MAG_LEN: u8 = 9;
const ACCEL_LSB_PER_G: f64 = 16384.0;
const GYRO_LSB_PER_DPS: f64 = 32.8;
const MAG_UT_PER_LSB: f64 = 0.15;
/// Untrimmed temperature sensitivity (ICM-20948 datasheet, p. 14).
const TEMP_LSB_PER_C: f64 = 333.87;
const GYRO_OFFSET_SAMPLES: u32 = 32;
/// HX..HZ byte-identical this many samples running means the field is
/// stale. On the live board each axis repeats between samples about 5 % of
/// the time (σ ≈ 7 LSB), so four identical samples happen by chance about
/// once in 10⁴ years at 1 Hz.
const STALL_SAMPLES: u32 = 4;
/// Samples between attempts to restart a stalled magnetometer.
const RESTART_EVERY_SAMPLES: u32 = 60;

/// One sample in the board frame, with the gyro offset removed.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Sample {
    accel_g: [f64; 3],
    gyro_dps: [f64; 3],
    mag_ut: Option<[f64; 3]>,
    temperature_c: f64,
}

pub(crate) struct Icm20948 {
    gyro_offset: [f64; 3],
    hard_iron_ut: Option<[f64; 3]>,
    watch: MagWatch,
}

/// Whether the magnetometer data is current. EXT_SLV_SENS_DATA keeps its
/// last bytes when the I2C master stops, so a stopped master looks like a
/// still field. ST1 DRDY can't tell: reading HX clears it, and the master
/// polls faster than the AK09916's 100 Hz, so most current reads show 0.
struct MagWatch {
    last: Option<[u8; 6]>,
    repeats: u32,
    since_restart: u32,
    stalled: bool,
}

impl MagWatch {
    fn new(stalled: bool) -> MagWatch {
        MagWatch { last: None, repeats: 0, since_restart: RESTART_EVERY_SAMPLES, stalled }
    }

    /// Records one sample's HX..HZ bytes and whether USER_CTRL still has
    /// the master enabled. True when the field is current. Once stalled,
    /// the field stays stale until its bytes change.
    fn observe(&mut self, field: [u8; 6], master_enabled: bool) -> bool {
        let changed = self.last.is_some_and(|last| last != field);
        self.repeats = if self.last == Some(field) { self.repeats + 1 } else { 0 };
        self.last = Some(field);
        self.since_restart = self.since_restart.saturating_add(1);
        let frozen = if self.stalled { !changed } else { self.repeats >= STALL_SAMPLES - 1 };
        self.stalled = !master_enabled || frozen;
        !self.stalled
    }

    /// True at most once every `RESTART_EVERY_SAMPLES` samples.
    fn restart_due(&mut self) -> bool {
        let due = self.since_restart >= RESTART_EVERY_SAMPLES;
        if due {
            self.since_restart = 0;
        }
        due
    }
}

fn select_bank(bus: &mut dyn Bus, bank: u8) -> Result<(), DriverError> {
    Ok(bus.write_byte(ADDR, REG_BANK_SEL, bank << 4)?)
}

/// One AK09916 register transfer through I2C_SLV4, which runs once per
/// enable. Returns the byte read (0 for a write). Leaves bank 0 selected.
fn aux_transfer(bus: &mut dyn Bus, addr: u8, reg: u8, write: Option<u8>) -> Result<u8, DriverError> {
    select_bank(bus, 3)?;
    bus.write_byte(ADDR, I2C_SLV4_ADDR, addr)?;
    bus.write_byte(ADDR, I2C_SLV4_REG, reg)?;
    if let Some(v) = write {
        bus.write_byte(ADDR, I2C_SLV4_DO, v)?;
    }
    bus.write_byte(ADDR, I2C_SLV4_CTRL, I2C_SLV_EN)?;
    select_bank(bus, 0)?;
    for _ in 0..50 {
        let status = bus.read_byte(ADDR, I2C_MST_STATUS)?;
        if status & I2C_SLV4_NACK != 0 {
            return Err(DriverError::Msg("AK09916 did not acknowledge"));
        }
        if status & I2C_SLV4_DONE != 0 {
            select_bank(bus, 3)?;
            let read = bus.read_byte(ADDR, I2C_SLV4_DI)?;
            select_bank(bus, 0)?;
            return Ok(read);
        }
        bus.sleep(Duration::from_millis(1));
    }
    Err(DriverError::Msg("AK09916 transfer timed out"))
}

/// Decodes a burst read. Every output is a signed 16-bit value: accel,
/// gyro and temperature big-endian, the magnetometer little-endian. The
/// AK09916's y and z axes point opposite the ICM's. A field beyond the
/// AK09916's ±4912 µT range (ST2 HOFL) leaves only the field unknown.
fn decode(b: &[u8; BURST_LEN], gyro_offset: [f64; 3]) -> Sample {
    let be = |i: usize| f64::from(i16::from_be_bytes([b[i], b[i + 1]]));
    let le = |i: usize| f64::from(i16::from_le_bytes([b[i], b[i + 1]]));
    let overflow = b[22] & ST2_HOFL != 0;
    Sample {
        accel_g: [0, 2, 4].map(|i| be(i) / ACCEL_LSB_PER_G),
        gyro_dps: [0, 1, 2].map(|k| (be(6 + 2 * k) - gyro_offset[k]) / GYRO_LSB_PER_DPS),
        mag_ut: (!overflow).then(|| [le(15), -le(17), -le(19)].map(|v| v * MAG_UT_PER_LSB)),
        temperature_c: be(12) / TEMP_LSB_PER_C + 21.0,
    }
}

impl Icm20948 {
    /// Resets the chip, configures accel ±2 g and gyro ±1000 dps with their
    /// low-pass filters, starts the magnetometer at 100 Hz behind the I2C
    /// master, and averages the gyro offset (keep the board still). Heading
    /// needs the compass's `hard_iron_ut` offsets.
    pub(crate) fn new(bus: &mut dyn Bus, hard_iron_ut: Option<[f64; 3]>) -> Result<Icm20948, DriverError> {
        // A previous process leaves the master polling the AK09916, and a
        // reset mid-transaction left the AK09916 unresponsive for 20 minutes.
        stop_master(bus)?;
        bus.write_byte(ADDR, PWR_MGMT_1, DEVICE_RESET)?;
        bus.sleep(Duration::from_millis(100));
        bus.write_byte(ADDR, PWR_MGMT_1, CLKSEL_AUTO)?;
        bus.sleep(Duration::from_millis(50));
        if bus.read_byte(ADDR, WHO_AM_I)? != 0xEA {
            return Err(DriverError::Msg("ICM20948 ID mismatch"));
        }

        select_bank(bus, 2)?;
        bus.write_byte(ADDR, GYRO_SMPLRT_DIV, 7)?; // 1.1 kHz / (1 + 7) = 137.5 Hz
        bus.write_byte(ADDR, GYRO_CONFIG_1, 0x35)?; // DLPF 6, ±1000 dps, DLPF on
        bus.write_byte(ADDR, ACCEL_SMPLRT_DIV_2, 7)?;
        bus.write_byte(ADDR, ACCEL_CONFIG, 0x31)?; // DLPF 6, ±2 g, DLPF on

        // Motion does not need the magnetometer, and the sampler retries it.
        let started = start_magnetometer(bus);
        if let Err(e) = &started {
            log(Self::NAME, format_args!("magnetometer not started: {e}"));
            select_bank(bus, 0)?;
        }

        let mut sum = [0.0; 3];
        for _ in 0..GYRO_OFFSET_SAMPLES {
            let raw = read_raw(bus)?;
            for (k, acc) in sum.iter_mut().enumerate() {
                *acc += f64::from(i16::from_be_bytes([raw[6 + 2 * k], raw[7 + 2 * k]]));
            }
            bus.sleep(Duration::from_millis(10));
        }
        let gyro_offset = sum.map(|s| s / f64::from(GYRO_OFFSET_SAMPLES));
        Ok(Icm20948 { gyro_offset, hard_iron_ut, watch: MagWatch::new(started.is_err()) })
    }

    /// One burst read, plus USER_CTRL to check that the I2C master still
    /// runs. Bank 0 stays selected after `new`, and no other thread
    /// addresses this chip, so no bank select is needed. A stale field
    /// reads `None` and restarts the magnetometer, rate-limited.
    pub(crate) fn read(&mut self, bus: &mut dyn Bus) -> Result<Motion, DriverError> {
        let raw = read_raw(bus)?;
        let master_enabled = bus.read_byte(ADDR, USER_CTRL)? & I2C_MST_EN != 0;
        let mut sample = decode(&raw, self.gyro_offset);
        let was_stalled = self.watch.stalled;
        let current = self.watch.observe(raw[15..21].try_into().unwrap(), master_enabled);
        if current && was_stalled {
            log(Self::NAME, "magnetometer recovered");
        }
        if !current {
            sample.mag_ut = None;
            if !was_stalled {
                log(Self::NAME, "magnetometer stalled; restarting it at most once a minute");
            }
            // A failed restart leaves the field None until a later one works,
            // but must not leave bank 3 selected for the next burst read.
            if self.watch.restart_due() && start_magnetometer(bus).is_err() {
                select_bank(bus, 0)?;
            }
        }
        Ok(motion(sample, self.hard_iron_ut))
    }
}

/// Disables SLV0 so the master finishes the read in flight and idles, then
/// disables the master. Leaves bank 0 selected.
fn stop_master(bus: &mut dyn Bus) -> Result<(), DriverError> {
    select_bank(bus, 3)?;
    bus.write_byte(ADDR, I2C_SLV0_CTRL, 0)?;
    select_bank(bus, 0)?;
    bus.write_byte(ADDR, USER_CTRL, 0)?;
    bus.sleep(Duration::from_millis(10));
    Ok(())
}

/// (Re)starts the I2C master and the AK09916 behind it, from any state:
/// the master is stopped before its reset, which the datasheet allows only
/// when no transaction is in flight. Leaves bank 0 selected on success.
fn start_magnetometer(bus: &mut dyn Bus) -> Result<(), DriverError> {
    stop_master(bus)?;
    bus.write_byte(ADDR, USER_CTRL, I2C_MST_RST)?;
    bus.sleep(Duration::from_millis(10));
    bus.write_byte(ADDR, USER_CTRL, I2C_MST_EN)?;
    select_bank(bus, 3)?;
    bus.write_byte(ADDR, I2C_MST_CTRL, 0x07)?; // clock code 7, which the datasheet recommends
    aux_transfer(bus, AK09916, CNTL3, Some(SOFT_RESET))?;
    bus.sleep(Duration::from_millis(10));
    let wia = [aux_transfer(bus, AK09916 | AUX_READ, WIA1, None)?, aux_transfer(bus, AK09916 | AUX_READ, WIA1 + 1, None)?];
    if wia != [0x48, 0x09] {
        return Err(DriverError::Msg("AK09916 ID mismatch"));
    }
    aux_transfer(bus, AK09916, CNTL2, Some(CONTINUOUS_100HZ))?;
    select_bank(bus, 3)?;
    bus.write_byte(ADDR, I2C_SLV0_ADDR, AK09916 | AUX_READ)?;
    bus.write_byte(ADDR, I2C_SLV0_REG, ST1)?;
    // Reading through ST2 releases the AK09916's data lock each cycle.
    bus.write_byte(ADDR, I2C_SLV0_CTRL, I2C_SLV_EN | MAG_LEN)?;
    select_bank(bus, 0)?;
    bus.sleep(Duration::from_millis(100));
    Ok(())
}

fn read_raw(bus: &mut dyn Bus) -> Result<[u8; BURST_LEN], DriverError> {
    let mut b = [0; BURST_LEN];
    bus.read_block(ADDR, ACCEL_XOUT_H, &mut b)?;
    Ok(b)
}

fn motion(s: Sample, hard_iron_ut: Option<[f64; 3]>) -> Motion {
    let (roll_deg, pitch_deg) = orientation::roll_pitch_deg(s.accel_g);
    let heading_deg = match (s.mag_ut, hard_iron_ut) {
        (Some(field), Some(offsets)) => orientation::heading_deg(s.accel_g, field, offsets),
        _ => None,
    };
    Motion { roll_deg, pitch_deg, heading_deg, accel_g: s.accel_g, gyro_dps: s.gyro_dps, mag_ut: s.mag_ut, temperature_c: s.temperature_c }
}

impl Chip for Icm20948 {
    type Group = Motion;
    const NAME: &'static str = "icm20948";
    const PERIOD: Duration = Duration::from_secs(1);

    fn sample(&mut self, bus: &mut dyn Bus) -> Result<Motion, DriverError> {
        self.read(bus)
    }

    fn publish(&self, reading: &mut Reading, motion: Option<Motion>) {
        reading.motion = motion;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::BusError;
    use crate::model::round;

    const BURST: [u8; BURST_LEN] = [
        0x40, 0x00, 0xC0, 0x00, 0xFF, 0x66, // accel 16384, -16384, -154
        0x01, 0x48, 0xFF, 0x66, 0x00, 0x00, // gyro 328, -154, 0
        0x0D, 0x05, // temperature 3333
        0x01, 0xC9, 0x00, 0x38, 0xFF, 0x64, 0x00, 0x00, 0x00, // ST1, HX 201, HY -200, HZ 100, TMPS, ST2
    ];

    #[test]
    fn decode_is_true_twos_complement() {
        let s = decode(&BURST, [0.0; 3]);
        assert_eq!(s.accel_g, [1.0, -1.0, -154.0 / 16384.0]);
        assert_eq!(s.gyro_dps, [10.0, -154.0 / 32.8, 0.0]);
        // 3333 / 333.87 + 21
        assert_eq!(round(s.temperature_c, 3), 30.983);
        // 201 * 0.15, then y and z negated into the ICM frame
        assert_eq!(s.mag_ut.map(|m| m.map(|v| round(v, 2))), Some([30.15, 30.0, -15.0]));
    }

    #[test]
    fn gyro_offset_is_subtracted_before_scaling() {
        let s = decode(&BURST, [10.0, 10.0, 10.0]);
        // (328 - 10) / 32.8, (-154 - 10) / 32.8 = -5.0 exactly (Waveshare's demo
        // decode, off by one in its sign handling, gave -4.97).
        assert_eq!(s.gyro_dps.map(|v| round(v, 9)), [9.695121951, -5.0, -0.304878049]);
    }

    #[test]
    fn a_magnetometer_overflow_loses_only_the_field_and_heading() {
        let mut b = BURST;
        b[22] = ST2_HOFL;
        let m = motion(decode(&b, [0.0; 3]), Some([0.0; 3]));
        assert_eq!((m.mag_ut, m.heading_deg), (None, None));
        assert_eq!((m.accel_g, m.gyro_dps[0]), ([1.0, -1.0, -154.0 / 16384.0], 10.0));
    }

    #[test]
    fn heading_waits_for_a_compass_calibration() {
        let level_facing_east =
            Sample { accel_g: [0.0, 0.0, 1.0], gyro_dps: [0.0; 3], mag_ut: Some([0.0, 20.0, -40.0]), temperature_c: 30.0 };
        assert_eq!(motion(level_facing_east, None).heading_deg, None);
        assert_eq!(motion(level_facing_east, Some([0.0; 3])).heading_deg, Some(90.0));
    }

    /// The ICM20948 as the driver sees it: banked registers, I2C_MST_STATUS
    /// fixed at `status`, and an AK09916 behind I2C_SLV4 holding `ak`. The
    /// AK09916 measures a new field (HX one higher) for every burst read, but
    /// EXT_SLV_SENS_DATA only takes it while USER_CTRL enables the master,
    /// SLV0 is enabled, and CNTL2 is in continuous mode. A soft reset puts
    /// the AK09916 in power-down, as on the chip. A device or master reset
    /// while the master reads the field hangs the aux bus: SLV4 never
    /// completes and the field stops updating.
    struct Imu {
        bank: u8,
        regs: HashMap<(u8, u8), u8>,
        ak: HashMap<u8, u8>,
        status: u8,
        status_reads: usize,
        hx: i16,
        master_resets: usize,
        /// A (bank, register) whose writes fail with EIO.
        failing_write: Option<(u8, u8)>,
        aux_hung: bool,
    }

    fn imu(status: u8, wia: [u8; 2]) -> Imu {
        let ak = HashMap::from([(WIA1, wia[0]), (WIA1 + 1, wia[1])]);
        let regs = HashMap::from([((0, WHO_AM_I), 0xEA)]);
        Imu { bank: 0, regs, ak, status, status_reads: 0, hx: 201, master_resets: 0, failing_write: None, aux_hung: false }
    }

    impl Imu {
        fn reg(&self, bank: u8, reg: u8) -> u8 {
            self.regs.get(&(bank, reg)).copied().unwrap_or(0)
        }

        fn master_reads_the_field(&self) -> bool {
            self.reg(0, USER_CTRL) & I2C_MST_EN != 0
                && self.reg(3, I2C_SLV0_CTRL) & I2C_SLV_EN != 0
                && self.ak.get(&CNTL2) == Some(&CONTINUOUS_100HZ)
        }
    }

    impl Bus for Imu {
        fn read_byte(&mut self, addr: u8, reg: u8) -> Result<u8, BusError> {
            assert_eq!(addr, ADDR);
            if (self.bank, reg) == (0, I2C_MST_STATUS) {
                self.status_reads += 1;
                return Ok(if self.aux_hung { 0 } else { self.status });
            }
            Ok(self.regs.get(&(self.bank, reg)).copied().unwrap_or(0))
        }
        fn write_byte(&mut self, addr: u8, reg: u8, value: u8) -> Result<(), BusError> {
            assert_eq!(addr, ADDR);
            if self.failing_write == Some((self.bank, reg)) {
                return Err(BusError(libc::EIO));
            }
            if reg == REG_BANK_SEL {
                self.bank = value >> 4;
                return Ok(());
            }
            let reset = matches!((self.bank, reg, value), (0, PWR_MGMT_1, DEVICE_RESET) | (0, USER_CTRL, I2C_MST_RST));
            if reset && self.master_reads_the_field() {
                self.aux_hung = true;
            }
            if (self.bank, reg, value) == (0, PWR_MGMT_1, DEVICE_RESET) {
                self.regs = HashMap::from([((0, WHO_AM_I), self.reg(0, WHO_AM_I))]);
                return Ok(());
            }
            self.regs.insert((self.bank, reg), value);
            if (self.bank, reg, value) == (0, USER_CTRL, I2C_MST_RST) {
                self.master_resets += 1;
            }
            if (self.bank, reg) == (3, I2C_SLV4_CTRL) && value & I2C_SLV_EN != 0 && !self.aux_hung {
                let target = self.regs[&(3, I2C_SLV4_ADDR)];
                let ak_reg = self.regs[&(3, I2C_SLV4_REG)];
                assert_eq!(target & !AUX_READ, AK09916);
                if target & AUX_READ != 0 {
                    let di = self.ak.get(&ak_reg).copied().unwrap_or(0);
                    self.regs.insert((3, I2C_SLV4_DI), di);
                } else {
                    let value = self.regs[&(3, I2C_SLV4_DO)];
                    self.ak.insert(ak_reg, value);
                    if (ak_reg, value) == (CNTL3, SOFT_RESET) {
                        self.ak.remove(&CNTL2);
                    }
                }
            }
            Ok(())
        }
        fn read_block(&mut self, _: u8, reg: u8, buf: &mut [u8]) -> Result<(), BusError> {
            assert_eq!((self.bank, reg), (0, ACCEL_XOUT_H));
            if self.master_reads_the_field() && !self.aux_hung {
                self.hx += 1;
            }
            buf.copy_from_slice(&BURST);
            buf[15..17].copy_from_slice(&self.hx.to_le_bytes());
            Ok(())
        }
        fn write_block(&mut self, _: u8, _: u8, _: &[u8]) -> Result<(), BusError> {
            unreachable!("the ICM20948 driver writes no blocks")
        }
        fn sleep(&mut self, _: Duration) {}
    }

    #[test]
    fn init_starts_the_magnetometer_behind_the_i2c_master() {
        let mut bus = imu(I2C_SLV4_DONE, [0x48, 0x09]);
        let icm = Icm20948::new(&mut bus, None).unwrap();
        assert_eq!((bus.ak[&CNTL3], bus.ak[&CNTL2]), (SOFT_RESET, CONTINUOUS_100HZ));
        // SLV0 reads ST1 onward, 9 bytes, every cycle.
        let slv0 = [I2C_SLV0_ADDR, I2C_SLV0_REG, I2C_SLV0_CTRL].map(|r| bus.regs[&(3, r)]);
        assert_eq!(slv0, [0x8C, 0x10, 0x89]);
        assert_eq!(bus.bank, 0, "sampling burst-reads bank 0 without selecting it");
        // The burst's raw gyro, 328, -154, 0, averaged as the offset.
        assert_eq!(icm.gyro_offset, [328.0, -154.0, 0.0]);
    }

    #[test]
    fn a_wrong_magnetometer_id_fails_its_start() {
        let mut bus = imu(I2C_SLV4_DONE, [0x48, 0x0A]);
        assert_eq!(start_magnetometer(&mut bus).err(), Some(DriverError::Msg("AK09916 ID mismatch")));
    }

    #[test]
    fn a_magnetometer_nack_fails_its_start() {
        let mut bus = imu(I2C_SLV4_NACK, [0x48, 0x09]);
        assert_eq!(start_magnetometer(&mut bus).err(), Some(DriverError::Msg("AK09916 did not acknowledge")));
        assert_eq!(bus.status_reads, 1);
    }

    #[test]
    fn a_magnetometer_that_never_finishes_times_out_after_50_polls() {
        let mut bus = imu(0, [0x48, 0x09]);
        assert_eq!(start_magnetometer(&mut bus).err(), Some(DriverError::Msg("AK09916 transfer timed out")));
        assert_eq!(bus.status_reads, 50);
    }

    #[test]
    fn a_wrong_icm_id_fails_init() {
        let mut bus = imu(I2C_SLV4_DONE, [0x48, 0x09]);
        bus.regs.insert((0, WHO_AM_I), 0xE9);
        assert_eq!(Icm20948::new(&mut bus, None).err(), Some(DriverError::Msg("ICM20948 ID mismatch")));
    }

    fn running_imu() -> (Icm20948, Imu) {
        let mut bus = imu(I2C_SLV4_DONE, [0x48, 0x09]);
        let mut icm = Icm20948::new(&mut bus, Some([0.0; 3])).unwrap();
        assert!(icm.read(&mut bus).unwrap().mag_ut.is_some());
        (icm, bus)
    }

    fn fields(icm: &mut Icm20948, bus: &mut Imu, n: usize) -> Vec<bool> {
        (0..n).map(|_| icm.read(bus).unwrap().mag_ut.is_some()).collect()
    }

    #[test]
    fn a_stopped_i2c_master_reads_none_until_the_restart_brings_it_back() {
        let (mut icm, mut bus) = running_imu();
        bus.regs.insert((0, USER_CTRL), 0); // another driver clears I2C_MST_EN
        let m = icm.read(&mut bus).unwrap();
        assert_eq!((m.mag_ut, m.heading_deg, m.accel_g[0]), (None, None, 1.0));
        assert_eq!(bus.master_resets, 2, "init, then one restart");
        assert_eq!(fields(&mut icm, &mut bus, 2), [true, true]);
    }

    #[test]
    fn a_frozen_field_reads_none_from_its_fourth_identical_sample() {
        let (mut icm, mut bus) = running_imu();
        bus.ak.insert(CNTL2, 0); // the AK09916 drops to power-down; the master runs on
        assert_eq!(fields(&mut icm, &mut bus, 4), [true, true, false, true]);
        assert_eq!(bus.master_resets, 2);
    }

    #[test]
    fn a_failing_restart_is_retried_once_a_minute_and_the_field_stays_none() {
        let (mut icm, mut bus) = running_imu();
        bus.regs.insert((0, USER_CTRL), 0);
        bus.status = I2C_SLV4_NACK; // the AK09916 no longer answers
        assert_eq!(fields(&mut icm, &mut bus, 121), [false; 121]);
        assert_eq!(bus.master_resets, 1 + 3, "init, then samples 1, 61 and 121");
    }

    #[test]
    fn a_restart_that_fails_in_bank_3_leaves_motion_readable() {
        let (mut icm, mut bus) = running_imu();
        bus.regs.insert((0, USER_CTRL), 0);
        bus.failing_write = Some((3, I2C_MST_CTRL));
        let m = icm.read(&mut bus).unwrap();
        assert_eq!((m.mag_ut, bus.bank), (None, 0));
        // The next burst reads bank 0: accel x is still 16384 LSB = 1 g.
        assert_eq!(icm.read(&mut bus).unwrap().accel_g[0], 1.0);
    }

    #[test]
    fn init_stops_a_master_left_polling_before_resetting_the_chip() {
        let mut bus = imu(I2C_SLV4_DONE, [0x48, 0x09]);
        // What the previous process left running.
        bus.regs.extend([((0, USER_CTRL), I2C_MST_EN), ((3, I2C_SLV0_CTRL), I2C_SLV_EN | MAG_LEN)]);
        bus.ak.insert(CNTL2, CONTINUOUS_100HZ);
        assert_eq!(Icm20948::new(&mut bus, None).err(), None);
        assert!(!bus.aux_hung);
    }

    #[test]
    fn a_magnetometer_that_fails_to_start_leaves_motion_and_is_retried_once_a_minute() {
        let mut bus = imu(0, [0x48, 0x09]); // SLV4 never completes
        let mut icm = Icm20948::new(&mut bus, None).unwrap();
        let m = icm.read(&mut bus).unwrap();
        assert_eq!((m.mag_ut, m.accel_g[0]), (None, 1.0));
        bus.status = I2C_SLV4_DONE; // the AK09916 answers again
        // Sample 1 retried at once; the next retry is due at sample 61.
        assert_eq!(fields(&mut icm, &mut bus, 61), [[false; 60].as_slice(), &[true]].concat());
    }
}
