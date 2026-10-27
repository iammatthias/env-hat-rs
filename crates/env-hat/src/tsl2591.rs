//! TSL2591 ambient light with auto-gain. Register names follow the ams
//! datasheet. The gain multipliers and the lux formula are community
//! constants from Adafruit's TSL2591 library, not datasheet values: the
//! datasheet gives typical gains of 24.5, 400 and 9200 (CH0) and no lux
//! equation. The lux is therefore uncalibrated.

use std::time::Duration;

use crate::hat::Chip;
use crate::reading::{Light, Reading};
use crate::{Bus, DriverError};

pub(crate) const ADDR: u8 = 0x29;
/// COMMAND: CMD set, normal register transaction.
const CMD: u8 = 0xA0;
const ENABLE: u8 = 0x00;
const PON: u8 = 0x01;
const AEN: u8 = 0x02;
const CONFIG: u8 = 0x01;
const PERSIST: u8 = 0x0C;
const ID: u8 = 0x12;
const C0DATAL: u8 = 0x14;
const ATIME_200MS: u8 = 0x01;
const OVERFLOW: DriverError = DriverError::Msg("TSL2591 saturated at the lowest gain");
const UNSETTLED: DriverError = DriverError::Msg("TSL2591 still saturated after 3 gain steps");

/// (CONFIG AGAIN bits, analog gain), low to high. Adafruit's nominal gains.
const GAINS: [(u8, f64); 4] = [(0x00, 1.0), (0x10, 25.0), (0x20, 428.0), (0x30, 9876.0)];
const START_GAIN: usize = 1;
/// 90 % of the 65535-count full scale at 200 ms.
const SATURATED: u16 = 58982;
const MAX_GAIN_STEPS: usize = 3;
/// Two integrations: the first may have started before a gain change.
const SETTLE: Duration = Duration::from_millis(450);

/// Which way auto-gain moves after one channel read.
#[derive(Debug, PartialEq, Eq)]
enum GainStep {
    Keep,
    Up,
    Down,
    /// Saturated at the lowest gain.
    Overflow,
}

fn gain_step(gain: usize, full: u16, ir: u16) -> GainStep {
    if full >= SATURATED || ir >= SATURATED {
        if gain == 0 { GainStep::Overflow } else { GainStep::Down }
    } else if full < 100 && gain + 1 < GAINS.len() {
        GainStep::Up
    } else {
        GainStep::Keep
    }
}

pub(crate) struct Tsl2591 {
    /// Index into `GAINS`.
    gain: usize,
}

fn write_reg(bus: &mut dyn Bus, reg: u8, val: u8) -> Result<(), DriverError> {
    Ok(bus.write_byte(ADDR, CMD | reg, val)?)
}

/// Counts per lux: integration time (ms) times analog gain over the
/// lux coefficient 408 from Adafruit's library.
fn cpl(gain: f64) -> f64 {
    (200.0 * gain) / 408.0
}

/// CH0 (full spectrum) and CH1 (infrared) counts.
fn channels(bus: &mut dyn Bus) -> Result<(u16, u16), DriverError> {
    let mut b = [0; 4];
    bus.read_block(ADDR, CMD | C0DATAL, &mut b)?;
    Ok((u16::from_le_bytes([b[0], b[1]]), u16::from_le_bytes([b[2], b[3]])))
}

impl Tsl2591 {
    pub(crate) fn new(bus: &mut dyn Bus) -> Result<Tsl2591, DriverError> {
        if bus.read_byte(ADDR, CMD | ID)? != 0x50 {
            return Err(DriverError::Msg("TSL2591 ID mismatch"));
        }
        // `Hat` does not use the interrupt, so it leaves interrupts off.
        write_reg(bus, ENABLE, PON | AEN)?;
        write_reg(bus, CONFIG, ATIME_200MS | GAINS[START_GAIN].0)?;
        write_reg(bus, PERSIST, 0x01)?;
        Ok(Tsl2591 { gain: START_GAIN })
    }

    pub(crate) fn read(&mut self, bus: &mut dyn Bus) -> Result<Light, DriverError> {
        let (full, ir) = self.auto_gain_channels(bus)?;
        let gain = GAINS[self.gain].1;
        let (f, i) = (f64::from(full), f64::from(ir));
        let lux = if full == 0 { 0.0 } else { ((f - i) * (1.00 - (i / f))) / cpl(gain) };
        Ok(Light { lux, full, ir, gain })
    }

    /// Reads the channels, stepping the gain at most `MAX_GAIN_STEPS` times
    /// until neither channel is saturated and full is at least 100 counts.
    fn auto_gain_channels(&mut self, bus: &mut dyn Bus) -> Result<(u16, u16), DriverError> {
        for _ in 0..MAX_GAIN_STEPS {
            let (full, ir) = channels(bus)?;
            let gain = match gain_step(self.gain, full, ir) {
                GainStep::Keep => return Ok((full, ir)),
                GainStep::Overflow => return Err(OVERFLOW),
                GainStep::Up => self.gain + 1,
                GainStep::Down => self.gain - 1,
            };
            write_reg(bus, CONFIG, ATIME_200MS | GAINS[gain].0)?;
            self.gain = gain;
            bus.sleep(SETTLE);
        }
        // Out of steps: dim counts are still a reading, saturated ones are not.
        let (full, ir) = channels(bus)?;
        match gain_step(self.gain, full, ir) {
            GainStep::Keep | GainStep::Up => Ok((full, ir)),
            GainStep::Down => Err(UNSETTLED),
            GainStep::Overflow => Err(OVERFLOW),
        }
    }
}

impl Chip for Tsl2591 {
    type Group = Light;
    const NAME: &'static str = "tsl2591";
    const PERIOD: Duration = Duration::from_secs(5);

    fn sample(&mut self, bus: &mut dyn Bus) -> Result<Light, DriverError> {
        self.read(bus)
    }

    fn publish(&self, reading: &mut Reading, light: Option<Light>) {
        reading.light = light;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Model;

    #[test]
    fn gain_steps() {
        assert_eq!(gain_step(1, 5000, 1000), GainStep::Keep);
        assert_eq!(gain_step(1, 0xFFFF, 1000), GainStep::Down);
        assert_eq!(gain_step(2, 30000, 58982), GainStep::Down);
        assert_eq!(gain_step(3, 30000, 0xFFFF), GainStep::Down);
        assert_eq!(gain_step(2, 58981, 58981), GainStep::Keep);
        assert_eq!(gain_step(1, 99, 0), GainStep::Up);
        assert_eq!(gain_step(1, 100, 0), GainStep::Keep);
        assert_eq!(gain_step(3, 5, 0), GainStep::Keep);
        assert_eq!(gain_step(0, 60000, 100), GainStep::Overflow);
        assert_eq!(gain_step(0, 50, 10), GainStep::Up);
    }

    fn tsl(channels: &[&[u8]]) -> (Tsl2591, Model) {
        let mut bus = Model::default().reg(ADDR, 0xB2, &[0x50]).block(ADDR, 0xB4, channels);
        let tsl = Tsl2591::new(&mut bus).unwrap();
        (tsl, bus)
    }

    #[test]
    fn a_wrong_id_says_so() {
        let mut bus = Model::default().reg(ADDR, 0xB2, &[0x51]);
        assert_eq!(Tsl2591::new(&mut bus).err(), Some(DriverError::Msg("TSL2591 ID mismatch")));
    }

    #[test]
    fn init_disables_interrupts() {
        let (_, bus) = tsl(&[&[0x10, 0x27, 0xE8, 0x03]]);
        assert_eq!(bus.ops[1..], ["wbd 29 a0 03", "wbd 29 a1 11", "wbd 29 ac 01"]);
    }

    #[test]
    fn dim_light_steps_gain_up_to_max() {
        let (mut tsl, mut bus) = tsl(&[&[50, 0, 10, 0]]);
        let l = tsl.read(&mut bus).unwrap();
        // (50 - 10) * (1 - 10/50) / (200 * 9876 / 408)
        assert_eq!((l.full, l.ir, l.gain, (l.lux * 1e6).round()), (50, 10, 9876.0, 6610.0));
        assert_eq!(bus.ops[4..], ["rbk 29 b4 4", "wbd 29 a1 21", "sleep 0.45", "rbk 29 b4 4", "wbd 29 a1 31", "sleep 0.45", "rbk 29 b4 4"]);
    }

    #[test]
    fn bright_light_steps_gain_down() {
        let (mut tsl, mut bus) = tsl(&[&[0xFF, 0xFF, 0x00, 0x10], &[0x10, 0x27, 0xE8, 0x03]]);
        let l = tsl.read(&mut bus).unwrap();
        // (10000 - 1000) * (1 - 0.1) / (200 * 1 / 408)
        assert_eq!((l.full, l.ir, l.gain, l.lux.round()), (10000, 1000, 1.0, 16524.0));
    }

    #[test]
    fn saturated_at_lowest_gain_is_an_overflow() {
        let (mut tsl, mut bus) = tsl(&[&[0xFF, 0xFF, 0x00, 0x10]]);
        assert_eq!(tsl.read(&mut bus), Err(OVERFLOW));
    }

    #[test]
    fn light_that_flickers_between_gains_runs_out_of_steps() {
        let (dim, saturated): (&[u8], &[u8]) = (&[50, 0, 10, 0], &[0xFF, 0xFF, 0x00, 0x10]);
        let (mut tsl, mut bus) = tsl(&[dim, saturated, dim, saturated]);
        assert_eq!(tsl.read(&mut bus).err(), Some(DriverError::Msg("TSL2591 still saturated after 3 gain steps")));
        assert_eq!(bus.ops.iter().filter(|o| o.starts_with("rbk")).count(), 4);
    }
}
