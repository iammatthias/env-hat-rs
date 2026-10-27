//! [`I2cDev`]: Linux i2c-dev through the `I2C_SLAVE` and `I2C_SMBUS`
//! ioctls, and [`SharedBus`], which shares one bus between threads.
//!
//! | transaction    | ioctl size                                              |
//! |----------------|---------------------------------------------------------|
//! | byte read/write | `I2C_SMBUS_BYTE_DATA` (2)                              |
//! | block read     | `I2C_SMBUS_I2C_BLOCK_DATA` (8); `_BROKEN` (6) for 32 bytes |
//! | block write    | `I2C_SMBUS_I2C_BLOCK_BROKEN` (6)                        |
//!
//! These are the sizes libi2c (and so python3-smbus) issues. `I2C_SLAVE` is
//! re-issued only when the target address changes.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::{Bus, BusError};

const I2C_SLAVE: libc::c_ulong = 0x0703;
const I2C_SMBUS: libc::c_ulong = 0x0720;
const I2C_SMBUS_READ: u8 = 1;
const I2C_SMBUS_WRITE: u8 = 0;
const I2C_SMBUS_BYTE_DATA: u32 = 2;
const I2C_SMBUS_I2C_BLOCK_BROKEN: u32 = 6;
const I2C_SMBUS_I2C_BLOCK_DATA: u32 = 8;
const I2C_SMBUS_BLOCK_MAX: usize = 32;

/// `union i2c_smbus_data`: byte / word / block[0] = length + 32 data + PEC.
#[repr(C, align(2))]
struct SmbusData {
    block: [u8; I2C_SMBUS_BLOCK_MAX + 2],
}

impl SmbusData {
    fn new() -> SmbusData {
        SmbusData { block: [0; I2C_SMBUS_BLOCK_MAX + 2] }
    }
}

/// `struct i2c_smbus_ioctl_data`.
#[repr(C)]
struct SmbusIoctlData {
    read_write: u8,
    command: u8,
    size: u32,
    data: *mut SmbusData,
}

/// An open `/dev/i2c-N`.
pub struct I2cDev {
    fd: libc::c_int,
    addr: Option<u8>,
}

impl I2cDev {
    /// Opens `/dev/i2c-{bus}`.
    pub fn open(bus: u32) -> Result<I2cDev, BusError> {
        let path = std::ffi::CString::new(format!("/dev/i2c-{bus}")).map_err(|_| BusError(libc::EINVAL))?;
        // SAFETY: path is a valid NUL-terminated string.
        let fd = unsafe { libc::open(path.as_ptr(), libc::O_RDWR | libc::O_CLOEXEC) };
        if fd < 0 {
            return Err(BusError::last_os());
        }
        Ok(I2cDev { fd, addr: None })
    }

    fn set_addr(&mut self, addr: u8) -> Result<(), BusError> {
        if self.addr != Some(addr) {
            // SAFETY: plain ioctl on an fd we own; arg is passed by value.
            if unsafe { libc::ioctl(self.fd, I2C_SLAVE as _, addr as libc::c_ulong) } < 0 {
                return Err(BusError::last_os());
            }
            self.addr = Some(addr);
        }
        Ok(())
    }

    fn access(&mut self, addr: u8, rw: u8, cmd: u8, size: u32, data: &mut SmbusData) -> Result<(), BusError> {
        self.set_addr(addr)?;
        let mut args = SmbusIoctlData { read_write: rw, command: cmd, size, data };
        // SAFETY: args and *data are valid for the duration of the call and
        // laid out as the kernel's i2c_smbus_ioctl_data / i2c_smbus_data.
        if unsafe { libc::ioctl(self.fd, I2C_SMBUS as _, &mut args as *mut SmbusIoctlData) } < 0 {
            return Err(BusError::last_os());
        }
        Ok(())
    }
}

impl Drop for I2cDev {
    fn drop(&mut self) {
        // SAFETY: closing an fd we opened and own.
        unsafe { libc::close(self.fd) };
    }
}

impl Bus for I2cDev {
    fn read_byte(&mut self, addr: u8, reg: u8) -> Result<u8, BusError> {
        let mut d = SmbusData::new();
        self.access(addr, I2C_SMBUS_READ, reg, I2C_SMBUS_BYTE_DATA, &mut d)?;
        Ok(d.block[0])
    }

    fn write_byte(&mut self, addr: u8, reg: u8, value: u8) -> Result<(), BusError> {
        let mut d = SmbusData::new();
        d.block[0] = value;
        self.access(addr, I2C_SMBUS_WRITE, reg, I2C_SMBUS_BYTE_DATA, &mut d)
    }

    fn read_block(&mut self, addr: u8, reg: u8, buf: &mut [u8]) -> Result<(), BusError> {
        if buf.is_empty() || buf.len() > I2C_SMBUS_BLOCK_MAX {
            return Err(BusError(libc::EINVAL));
        }
        let mut d = SmbusData::new();
        d.block[0] = buf.len() as u8;
        let size = if buf.len() == I2C_SMBUS_BLOCK_MAX { I2C_SMBUS_I2C_BLOCK_BROKEN } else { I2C_SMBUS_I2C_BLOCK_DATA };
        self.access(addr, I2C_SMBUS_READ, reg, size, &mut d)?;
        if d.block[0] as usize != buf.len() {
            return Err(BusError(libc::EIO));
        }
        buf.copy_from_slice(&d.block[1..=buf.len()]);
        Ok(())
    }

    fn write_block(&mut self, addr: u8, reg: u8, data: &[u8]) -> Result<(), BusError> {
        if data.is_empty() || data.len() > I2C_SMBUS_BLOCK_MAX {
            return Err(BusError(libc::EINVAL));
        }
        let mut d = SmbusData::new();
        d.block[0] = data.len() as u8;
        d.block[1..=data.len()].copy_from_slice(data);
        self.access(addr, I2C_SMBUS_WRITE, reg, I2C_SMBUS_I2C_BLOCK_BROKEN, &mut d)
    }
}

/// One bus shared by the sampler threads. Each call holds the lock for one
/// transaction (with `I2cDev`'s slave-address cache inside it). `sleep` is
/// the default, which takes no lock, so one chip's conversion wait never
/// delays another chip.
pub(crate) struct SharedBus<B>(Arc<Mutex<B>>);

impl<B> SharedBus<B> {
    pub(crate) fn new(bus: B) -> SharedBus<B> {
        SharedBus(Arc::new(Mutex::new(bus)))
    }

    fn lock(&self) -> MutexGuard<'_, B> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl<B> Clone for SharedBus<B> {
    fn clone(&self) -> SharedBus<B> {
        SharedBus(Arc::clone(&self.0))
    }
}

impl<B: Bus> Bus for SharedBus<B> {
    fn read_byte(&mut self, addr: u8, reg: u8) -> Result<u8, BusError> {
        self.lock().read_byte(addr, reg)
    }

    fn write_byte(&mut self, addr: u8, reg: u8, value: u8) -> Result<(), BusError> {
        self.lock().write_byte(addr, reg, value)
    }

    fn read_block(&mut self, addr: u8, reg: u8, buf: &mut [u8]) -> Result<(), BusError> {
        self.lock().read_block(addr, reg, buf)
    }

    fn write_block(&mut self, addr: u8, reg: u8, data: &[u8]) -> Result<(), BusError> {
        self.lock().write_block(addr, reg, data)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    /// Answers every read with the address it was sent to.
    struct Echo;

    impl Bus for Echo {
        fn read_byte(&mut self, addr: u8, _: u8) -> Result<u8, BusError> {
            Ok(addr)
        }
        fn write_byte(&mut self, _: u8, _: u8, _: u8) -> Result<(), BusError> {
            Ok(())
        }
        fn read_block(&mut self, addr: u8, _: u8, buf: &mut [u8]) -> Result<(), BusError> {
            buf.fill(addr);
            Ok(())
        }
        fn write_block(&mut self, _: u8, _: u8, _: &[u8]) -> Result<(), BusError> {
            Ok(())
        }
    }

    #[test]
    fn a_sleeping_chip_does_not_hold_the_bus() {
        let bus = SharedBus::new(Echo);
        let mut sleeper = bus.clone();
        let t = std::thread::spawn(move || sleeper.sleep(Duration::from_secs(1)));
        std::thread::sleep(Duration::from_millis(50));
        let start = Instant::now();
        assert_eq!(bus.clone().read_byte(0x59, 0), Ok(0x59));
        assert!(start.elapsed() < Duration::from_millis(500), "read waited {:?}", start.elapsed());
        t.join().unwrap();
    }
}
