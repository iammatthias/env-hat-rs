//! Test support: a register-model bus for driver tests, in which chips
//! answer from a register map and every transaction is logged, plus helpers
//! shared by the unit tests.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Duration;

use crate::{Bus, BusError};

/// Chips answer from a register map; unknown addresses NACK. Byte and block
/// reads pop a scripted response queue when one exists (the last entry
/// repeats), else read the map, block reads from consecutive registers.
/// Byte writes store into the register map, except that the write named by
/// `nack` fails (EIO) on its given 1-based occurrence.
#[derive(Default)]
pub(crate) struct Model {
    pub(crate) regs: HashMap<(u8, u8), u8>,
    pub(crate) bytes: HashMap<(u8, u8), VecDeque<u8>>,
    pub(crate) blocks: HashMap<(u8, u8), VecDeque<Vec<u8>>>,
    pub(crate) ops: Vec<String>,
    pub(crate) nack: Option<(&'static str, usize)>,
}

fn pop<T: Clone>(q: &mut VecDeque<T>) -> T {
    if q.len() > 1 { q.pop_front().unwrap() } else { q[0].clone() }
}

impl Model {
    pub(crate) fn reg(mut self, addr: u8, first: u8, values: &[u8]) -> Model {
        for (&v, r) in values.iter().zip(first..) {
            self.regs.insert((addr, r), v);
        }
        self
    }

    pub(crate) fn bytes(mut self, addr: u8, reg: u8, responses: &[u8]) -> Model {
        self.bytes.insert((addr, reg), responses.iter().copied().collect());
        self
    }

    pub(crate) fn block(mut self, addr: u8, cmd: u8, responses: &[&[u8]]) -> Model {
        self.blocks.insert((addr, cmd), responses.iter().map(|r| r.to_vec()).collect());
        self
    }

    fn present(&self, addr: u8) -> Result<(), BusError> {
        let known = self.regs.keys().chain(self.bytes.keys()).chain(self.blocks.keys()).any(|&(a, _)| a == addr);
        if known { Ok(()) } else { Err(BusError(121)) }
    }
}

impl Bus for Model {
    fn read_byte(&mut self, addr: u8, reg: u8) -> Result<u8, BusError> {
        self.present(addr)?;
        self.ops.push(format!("rbd {addr:02x} {reg:02x}"));
        if let Some(q) = self.bytes.get_mut(&(addr, reg)) {
            return Ok(pop(q));
        }
        Ok(self.regs.get(&(addr, reg)).copied().unwrap_or(0))
    }

    fn write_byte(&mut self, addr: u8, reg: u8, value: u8) -> Result<(), BusError> {
        self.present(addr)?;
        let op = format!("wbd {addr:02x} {reg:02x} {value:02x}");
        let nth = self.ops.iter().filter(|o| **o == op).count() + 1;
        self.ops.push(op.clone());
        if self.nack == Some((op.as_str(), nth)) {
            return Err(BusError(5));
        }
        self.regs.insert((addr, reg), value);
        Ok(())
    }

    fn read_block(&mut self, addr: u8, reg: u8, buf: &mut [u8]) -> Result<(), BusError> {
        self.present(addr)?;
        self.ops.push(format!("rbk {addr:02x} {reg:02x} {}", buf.len()));
        if let Some(q) = self.blocks.get_mut(&(addr, reg)) {
            buf.copy_from_slice(&pop(q));
            return Ok(());
        }
        for (b, r) in buf.iter_mut().zip(reg..) {
            *b = self.regs.get(&(addr, r)).copied().unwrap_or(0);
        }
        Ok(())
    }

    fn write_block(&mut self, addr: u8, reg: u8, data: &[u8]) -> Result<(), BusError> {
        self.present(addr)?;
        self.ops.push(format!("wbk {addr:02x} {reg:02x} {data:02x?}"));
        Ok(())
    }

    fn sleep(&mut self, duration: Duration) {
        self.ops.push(format!("sleep {}", duration.as_secs_f64()));
    }
}

/// `x` rounded to `places` decimals.
pub(crate) fn round(x: f64, places: i32) -> f64 {
    let k = 10f64.powi(places);
    (x * k).round() / k
}

/// A fresh, empty directory for one test.
pub(crate) fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("env-hat-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}
