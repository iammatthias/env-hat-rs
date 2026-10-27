//! The Waveshare Environment Sensor HAT (BME280, TSL2591, LTR390, SGP40,
//! ICM20948) on a Raspberry Pi, over Linux i2c-dev.
//!
//! [`Hat::start`] runs one sampler thread per chip at that chip's natural
//! rate and keeps the newest value of each in a [`Reading`]. Each group is
//! `None` until its chip's first sample, so read it through [`Hat::every`]:
//!
//! ```no_run
//! use std::time::Duration;
//!
//! let hat = env_hat::Hat::start(env_hat::Config::default())?;
//! for reading in hat.every(Duration::from_secs(10)) {
//!     if let Some(air) = reading.air {
//!         println!("{:.1} °C", air.temperature_c);
//!     }
//! }
//! # Ok::<(), env_hat::Error>(())
//! ```
//!
//! The guides are in the repository's `docs/` directory. `hardware.md`
//! covers the board and how each chip is configured. `api.md` covers every
//! type and field. `accuracy.md` covers how each value is computed and
//! which test checks it. `recording.md` lists the names to record each value
//! under, from [`Reading::values`].

use std::fmt;
use std::time::Duration;

mod air;
mod bme280;
pub mod gas_index;
mod hat;
mod i2c;
mod icm20948;
mod ltr390;
#[cfg(test)]
mod model;
mod orientation;
mod reading;
pub mod schema;
mod sgp40;
mod state;
mod tsl2591;

pub use hat::{Config, Error, Hat, StopHandle};
pub use i2c::I2cDev;
pub use reading::{Air, Gas, Light, Motion, Reading, Uv};
pub use schema::Field;

/// The four register transactions the drivers need from an I2C bus, plus a
/// default `sleep`. [`I2cDev`] implements it on Linux. Tests and
/// instrumentation implement their own and pass it to [`Hat::start_on`].
pub trait Bus {
    /// Reads the register `reg` of the chip at `addr`.
    fn read_byte(&mut self, addr: u8, reg: u8) -> Result<u8, BusError>;
    /// Writes `value` to the register `reg` of the chip at `addr`.
    fn write_byte(&mut self, addr: u8, reg: u8, value: u8) -> Result<(), BusError>;
    /// Reads `buf.len()` consecutive bytes, 1 to 32, starting at `reg`.
    fn read_block(&mut self, addr: u8, reg: u8, buf: &mut [u8]) -> Result<(), BusError>;
    /// Writes `reg`, then `data`, 1 to 32 bytes, in one transaction.
    fn write_block(&mut self, addr: u8, reg: u8, data: &[u8]) -> Result<(), BusError>;
    /// A driver's wait for a conversion. [`Hat`] does not forward it to your
    /// bus. Its samplers always sleep the thread, outside the bus lock, so
    /// an override has no effect under a `Hat`.
    fn sleep(&mut self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// An errno from a bus transaction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BusError(pub i32);

impl BusError {
    /// The errno of the last failed system call on this thread, or EIO.
    pub fn last_os() -> BusError {
        BusError(std::io::Error::last_os_error().raw_os_error().unwrap_or(libc::EIO))
    }
}

impl fmt::Display for BusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        std::io::Error::from_raw_os_error(self.0).fmt(f)
    }
}

impl std::error::Error for BusError {}

/// Why a driver failed to start or to sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DriverError {
    Bus(BusError),
    Msg(&'static str),
}

impl From<BusError> for DriverError {
    fn from(e: BusError) -> Self {
        DriverError::Bus(e)
    }
}

impl fmt::Display for DriverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DriverError::Bus(e) => e.fmt(f),
            DriverError::Msg(m) => f.write_str(m),
        }
    }
}

/// Compiles the README's example as a doctest.
#[cfg(doctest)]
#[doc = include_str!("../../../README.md")]
struct ReadmeDoctests;
