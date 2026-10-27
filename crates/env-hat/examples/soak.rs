//! Runs the Hat on the live bus and reports bus errors per address, SGP40
//! measurement spacing, and motion sanity checks. The first argument is the
//! duration in seconds (default 330). The optional second is a state
//! directory for the VOC baseline.
//!
//!     ./soak 330 /tmp/soak-state

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use env_hat::{Bus, BusError, Config, Hat, I2cDev, Reading};

#[derive(Default)]
struct Log {
    errors: Vec<(u8, BusError)>,
    sgp40_measures: Vec<Instant>,
}

/// Counts failed transactions and timestamps SGP40 measure commands.
struct Traced {
    bus: I2cDev,
    log: Arc<Mutex<Log>>,
}

impl Traced {
    fn note<T>(&self, addr: u8, r: Result<T, BusError>) -> Result<T, BusError> {
        if let Err(e) = &r {
            self.log.lock().unwrap().errors.push((addr, *e));
        }
        r
    }
}

impl Bus for Traced {
    fn read_byte(&mut self, addr: u8, reg: u8) -> Result<u8, BusError> {
        let r = self.bus.read_byte(addr, reg);
        self.note(addr, r)
    }
    fn write_byte(&mut self, addr: u8, reg: u8, value: u8) -> Result<(), BusError> {
        let r = self.bus.write_byte(addr, reg, value);
        self.note(addr, r)
    }
    fn read_block(&mut self, addr: u8, reg: u8, buf: &mut [u8]) -> Result<(), BusError> {
        let r = self.bus.read_block(addr, reg, buf);
        self.note(addr, r)
    }
    fn write_block(&mut self, addr: u8, reg: u8, data: &[u8]) -> Result<(), BusError> {
        if addr == 0x59 && reg == 0x26 {
            self.log.lock().unwrap().sgp40_measures.push(Instant::now());
        }
        let r = self.bus.write_block(addr, reg, data);
        self.note(addr, r)
    }
}

fn norm(v: [f64; 3]) -> f64 {
    v.iter().map(|x| x * x).sum::<f64>().sqrt()
}

fn range(v: &[f64]) -> (f64, f64) {
    v.iter().fold((f64::MAX, f64::MIN), |(lo, hi), &x| (lo.min(x), hi.max(x)))
}

/// One line per snapshot, with `-` for a value the HAT has not produced.
fn line(r: &Reading) -> String {
    let show = |v: Option<f64>, digits: usize| v.map_or("-".to_string(), |v| format!("{v:.digits$}"));
    let air = r.air.as_ref();
    let motion = r.motion.as_ref();
    format!(
        "{} °C  {} %RH  {} hPa  {} lux  UV {}  VOC {}  raw {}  |a| {} g  |B| {} µT",
        show(air.map(|a| a.temperature_c), 2),
        show(air.map(|a| a.humidity_pct), 1),
        show(air.map(|a| a.pressure_hpa), 2),
        show(r.light.map(|l| l.lux), 2),
        show(r.uv.map(|u| u.index), 2),
        show(r.gas.and_then(|g| g.voc_index).map(f64::from), 0),
        show(r.gas.map(|g| f64::from(g.raw)), 0),
        show(motion.map(|m| norm(m.accel_g)), 4),
        show(motion.and_then(|m| m.mag_ut).map(norm), 1),
    )
}

fn main() {
    let secs: u64 = std::env::args().nth(1).map_or(330, |s| s.parse().expect("seconds"));
    let log = Arc::new(Mutex::new(Log::default()));
    let state_dir = std::env::args().nth(2).map(Into::into);
    let config = Config { state_dir, altitude_m: Some(100.0), ..Config::default() };
    let hat = Hat::start_on(Traced { bus: I2cDev::open(1).expect("open /dev/i2c-1"), log: log.clone() }, config).expect("start");

    let start = Instant::now();
    let (mut accel, mut field, mut snapshots, mut mag_absent) = (Vec::new(), Vec::new(), 0, 0);
    while start.elapsed() < Duration::from_secs(secs) {
        std::thread::sleep(Duration::from_secs(10));
        let r = hat.latest();
        if let Some(m) = &r.motion {
            snapshots += 1;
            accel.push(norm(m.accel_g));
            match m.mag_ut {
                Some(b) => field.push(norm(b)),
                None => mag_absent += 1,
            }
        }
        println!("t={:>4}s {}", start.elapsed().as_secs(), line(&r));
    }
    hat.stop();

    let log = log.lock().unwrap();
    println!("bus errors: {}", log.errors.len());
    for (addr, e) in &log.errors {
        println!("  0x{addr:02x}: {e}");
    }
    let gaps: Vec<f64> = log.sgp40_measures.windows(2).map(|w| (w[1] - w[0]).as_secs_f64()).collect();
    let within = gaps.iter().filter(|g| (0.9..=1.1).contains(*g)).count();
    let (lo, hi) = range(&gaps);
    println!(
        "sgp40 intervals: {}, within 1.0 ± 0.1 s: {within} ({:.2} %), min {lo:.4} s, max {hi:.4} s",
        gaps.len(),
        100.0 * within as f64 / gaps.len().max(1) as f64
    );
    println!("|accel| g over {} snapshots: {:.4?}", accel.len(), range(&accel));
    println!("|B| µT over {} snapshots: {:.1?}", field.len(), range(&field));
    println!("magnetometer absent in {mag_absent} of {snapshots} motion snapshots");
}
