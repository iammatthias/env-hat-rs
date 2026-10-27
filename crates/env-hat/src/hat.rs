//! [`Hat`]: one sampler thread per chip on a shared bus, each publishing its
//! group into one snapshot that readers copy without touching the bus.

use std::fmt;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::air;
use crate::bme280::Bme280;
use crate::gas_index::VocIndex;
use crate::i2c::{I2cDev, SharedBus};
use crate::icm20948::Icm20948;
use crate::ltr390::Ltr390;
use crate::reading::{Air, Gas, Reading};
use crate::sgp40::Sgp40;
use crate::tsl2591::Tsl2591;
use crate::{Bus, BusError, DriverError};

/// How to run the HAT. Every field has a default.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// The `N` in `/dev/i2c-N`.
    pub bus: u32,
    /// Where the VOC Index baseline persists across restarts.
    pub state_dir: Option<PathBuf>,
    /// The magnetometer's hard-iron offsets in µT. `Motion::heading_deg` is
    /// `None` without them.
    pub hard_iron_ut: Option<[f64; 3]>,
    /// Added to the BME280's temperature, in °C. 0 by default, which
    /// reports the measurement as is. To compensate for the Pi's heat, set
    /// it to a reference thermometer's reading next to the board minus
    /// `Air::temperature_c`. The Pi's heat varies with its load, so a
    /// constant offset holds only at a similar load.
    pub temperature_offset_c: f64,
    /// Adds sea-level pressure when set.
    pub altitude_m: Option<f64>,
}

impl Default for Config {
    fn default() -> Config {
        Config { bus: 1, state_dir: None, hard_iron_ut: None, temperature_offset_c: 0.0, altitude_m: None }
    }
}

/// Why [`Hat::start`] failed.
#[derive(Debug)]
pub enum Error {
    /// [`Hat::start`] could not open the I2C bus device.
    Bus(BusError),
    /// The OS could not spawn a sampler thread.
    Thread(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Bus(e) => write!(f, "opening the i2c bus: {e}"),
            Error::Thread(e) => write!(f, "starting a sampler thread: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// The running HAT. Dropping it (or [`Hat::stop`]) stops the samplers.
pub struct Hat {
    latest: Shared,
    stop: Arc<Stop>,
    threads: Vec<JoinHandle<()>>,
}

impl Hat {
    /// Opens `/dev/i2c-{bus}` and starts one sampler thread per chip. A chip
    /// that fails to start logs a line on stderr, and its group stays
    /// `None`.
    pub fn start(config: Config) -> Result<Hat, Error> {
        Hat::start_on(I2cDev::open(config.bus).map_err(Error::Bus)?, config)
    }

    /// [`Hat::start`] on any [`Bus`]: for tests and bus instrumentation.
    pub fn start_on<B: Bus + Send + 'static>(bus: B, config: Config) -> Result<Hat, Error> {
        let bus = SharedBus::new(bus);
        let mut hat = Hat { latest: Shared::default(), stop: Arc::default(), threads: Vec::new() };
        let Config { temperature_offset_c, altitude_m, state_dir, hard_iron_ut, .. } = config;
        hat.spawn(&bus, move |bus| Ok(AirChip { bme: Bme280::new(bus)?, offset_c: temperature_offset_c, altitude_m }))?;
        hat.spawn(&bus, Tsl2591::new)?;
        hat.spawn(&bus, Ltr390::new)?;
        let latest = hat.latest.clone();
        hat.spawn(&bus, move |bus| Ok(GasChip::new(Sgp40::new(bus)?, latest, temperature_offset_c, state_dir, unix_ms())))?;
        hat.spawn(&bus, move |bus| Icm20948::new(bus, hard_iron_ut))?;
        Ok(hat)
    }

    /// The newest value from every chip. Never touches the bus.
    pub fn latest(&self) -> Reading {
        let mut r = lock(&self.latest).clone();
        r.unix_ms = unix_ms();
        r
    }

    /// A [`Hat::latest`] snapshot at every multiple of `period` on the wall
    /// clock. For a 30 s period, that is at :00 and :30 past each minute. A
    /// slow consumer skips slots rather than receiving a burst. The iterator
    /// ends when a [`StopHandle`] stops the `Hat`.
    pub fn every(&self, period: Duration) -> impl Iterator<Item = Reading> + '_ {
        let period_ms = (period.as_millis() as u64).max(1);
        std::iter::from_fn(move || {
            let due = Instant::now() + Duration::from_millis(until_aligned(unix_ms(), period_ms));
            (!self.stop.wait_until(due)).then(|| self.latest())
        })
    }

    /// A handle that stops the `Hat` from another thread, such as one that
    /// waits for SIGTERM.
    pub fn stop_handle(&self) -> StopHandle {
        StopHandle(self.stop.clone())
    }

    /// Stops the samplers and waits for any sample in progress. If
    /// `Config::state_dir` is set, the SGP40 sampler then saves the VOC
    /// baseline, provided it has learned for 3 hours or was restored.
    pub fn stop(self) {
        drop(self);
    }

    fn spawn<C: Chip, B: Bus + Send + 'static>(
        &mut self,
        bus: &SharedBus<B>,
        init: impl FnOnce(&mut dyn Bus) -> Result<C, DriverError> + Send + 'static,
    ) -> Result<(), Error> {
        let (mut bus, latest, stop) = (bus.clone(), self.latest.clone(), self.stop.clone());
        let run = move || {
            let mut chip = match init(&mut bus) {
                Ok(chip) => chip,
                Err(e) => return log(C::NAME, format_args!("not started: {e}")),
            };
            let mut due = Instant::now();
            let mut errors = ErrorLog::default();
            loop {
                let group = chip.sample(&mut bus).map_err(|e| {
                    if let Some(line) = errors.line(e.to_string(), Instant::now()) {
                        log(C::NAME, format_args!("read error: {line}"));
                    }
                });
                chip.publish(&mut lock(&latest), group.ok());
                due = next_slot(due, C::PERIOD, Instant::now());
                if stop.wait_until(due) {
                    return chip.stop();
                }
            }
        };
        let thread = thread::Builder::new().name(C::NAME.into()).stack_size(STACK_BYTES).spawn(run);
        self.threads.push(thread.map_err(Error::Thread)?);
        Ok(())
    }
}

/// Stops a [`Hat`] from any thread. The samplers finish their current sample
/// and stop, and [`Hat::every`] ends. Call [`Hat::stop`] afterwards to wait
/// for them.
#[derive(Clone)]
pub struct StopHandle(Arc<Stop>);

impl StopHandle {
    /// Asks the samplers to stop and ends [`Hat::every`]. Returns at once.
    pub fn stop(&self) {
        self.0.set();
    }
}

impl Drop for Hat {
    fn drop(&mut self) {
        self.stop.set();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

/// Writes one line for `chip` to stderr. Every message the crate logs goes
/// through here.
pub(crate) fn log(chip: &str, message: impl fmt::Display) {
    eprintln!("env-hat: {chip}: {message}");
}

const STACK_BYTES: usize = 128 * 1024;
const REPEAT_LOG_EVERY: Duration = Duration::from_secs(10 * 60);

/// Rate-limits one sampler's read errors, so a chip failing every second
/// does not fill the journal. A new error is logged at once; the same error
/// again at most every 10 minutes, with how often it was suppressed.
#[derive(Default)]
struct ErrorLog {
    last: Option<(String, Instant)>,
    suppressed: u32,
}

impl ErrorLog {
    /// The line to log for `error` at `now`, if any.
    fn line(&mut self, error: String, now: Instant) -> Option<String> {
        let line = match &self.last {
            Some((last, at)) if *last == error => {
                if now.duration_since(*at) < REPEAT_LOG_EVERY {
                    self.suppressed += 1;
                    return None;
                }
                format!("{error} (repeated {} times since last logged)", self.suppressed)
            }
            _ => error.clone(),
        };
        self.last = Some((error, now));
        self.suppressed = 0;
        Some(line)
    }
}

type Shared = Arc<Mutex<Reading>>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn unix_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Milliseconds from `now_ms` to the next multiple of `period_ms`.
fn until_aligned(now_ms: u64, period_ms: u64) -> u64 {
    period_ms - now_ms % period_ms
}

/// The slot after `due` on a fixed-rate schedule. Slots that have already
/// passed are dropped, never replayed in a burst: the VOC algorithm assumes
/// its nominal spacing.
fn next_slot(due: Instant, period: Duration, now: Instant) -> Instant {
    let next = due + period;
    if next > now {
        return next;
    }
    let missed = ((now - next).as_nanos() / period.as_nanos()) as u32 + 1;
    next + period * missed
}

#[derive(Default)]
struct Stop {
    stopped: Mutex<bool>,
    wake: Condvar,
}

impl Stop {
    fn set(&self) {
        *lock(&self.stopped) = true;
        self.wake.notify_all();
    }

    /// Sleeps until `deadline`; true if stopped meanwhile.
    fn wait_until(&self, deadline: Instant) -> bool {
        let mut stopped = lock(&self.stopped);
        while !*stopped {
            let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) else {
                return false;
            };
            stopped = self.wake.wait_timeout(stopped, left).unwrap_or_else(PoisonError::into_inner).0;
        }
        true
    }
}

/// One chip's sampler: what it measures, how often, and where it publishes.
/// Each chip is the only writer of the fields it publishes.
pub(crate) trait Chip: Send + 'static {
    type Group: Send;
    const NAME: &'static str;
    const PERIOD: Duration;
    fn sample(&mut self, bus: &mut dyn Bus) -> Result<Self::Group, DriverError>;
    /// Writes the newest sample, `None` if it failed, into the snapshot.
    fn publish(&self, reading: &mut Reading, group: Option<Self::Group>);
    /// Runs once when the `Hat` stops.
    fn stop(&mut self) {}
}

/// The BME280's air, moved by the configured temperature offset.
struct AirChip {
    bme: Bme280,
    offset_c: f64,
    altitude_m: Option<f64>,
}

impl Chip for AirChip {
    type Group = Air;
    const NAME: &'static str = "bme280";
    const PERIOD: Duration = Duration::from_secs(2);

    fn sample(&mut self, bus: &mut dyn Bus) -> Result<Air, DriverError> {
        let board = self.bme.read(bus)?;
        if board.humidity_pct <= 0.0 {
            return Err(DriverError::Msg("humidity reads 0 %"));
        }
        let (temperature_c, humidity_pct) = air::offset(board.temperature_c, board.humidity_pct, self.offset_c);
        Ok(Air {
            temperature_c,
            humidity_pct,
            pressure_hpa: board.pressure_hpa,
            dew_point_c: air::dew_point_c(board.temperature_c, board.humidity_pct),
            sea_level_hpa: self.altitude_m.map(|h| air::sea_level_hpa(board.pressure_hpa, h, temperature_c)),
        })
    }

    fn publish(&self, reading: &mut Reading, air: Option<Air>) {
        reading.air = air;
    }
}

struct GasChip {
    sgp: Sgp40,
    voc: VocIndex,
    /// Read for the BME280's air, which is what reaches the SGP40.
    latest: Shared,
    /// Taken back off that air, because the SGP40 sits in the board's air.
    offset_c: f64,
    state_dir: Option<PathBuf>,
    /// Samples fed to `voc`, i.e. seconds of operation.
    uptime_s: u32,
    restored: bool,
    saved_at_s: u32,
}

const VOC_LEARNING_S: u32 = 3 * 3600;
const VOC_SAVE_EVERY_S: u32 = 10 * 60;

impl GasChip {
    fn new(sgp: Sgp40, latest: Shared, offset_c: f64, state_dir: Option<PathBuf>, now_ms: u64) -> GasChip {
        let mut voc = VocIndex::default();
        let saved = state_dir.as_deref().and_then(|d| crate::state::load_voc(d, now_ms));
        if let Some((mean, std)) = saved {
            voc.set_state(mean, std);
        }
        let mut chip = GasChip { sgp, voc, latest, offset_c, state_dir, uptime_s: 0, restored: saved.is_some(), saved_at_s: 0 };
        // Refreshes the file's time, so a crash in the next 10 minutes still
        // leaves a baseline young enough to restore.
        if chip.restored {
            chip.save(now_ms);
        }
        chip
    }

    /// Counts one second of operation and saves the baseline when due.
    fn tick(&mut self, now_ms: u64) {
        self.uptime_s += 1;
        if self.uptime_s - self.saved_at_s >= VOC_SAVE_EVERY_S {
            self.save(now_ms);
        }
    }

    /// Saves the baseline if it is trustworthy: restored, or learned for
    /// 3 h as Sensirion requires.
    fn save(&mut self, now_ms: u64) {
        let Some(dir) = &self.state_dir else { return };
        if !self.restored && self.uptime_s < VOC_LEARNING_S {
            return;
        }
        self.saved_at_s = self.uptime_s;
        if let Err(e) = crate::state::save_voc(dir, now_ms, self.voc.state()) {
            log(Self::NAME, format_args!("saving the VOC state in {}: {e}", dir.display()));
        }
    }
}

impl Chip for GasChip {
    type Group = Gas;
    const NAME: &'static str = "sgp40";
    const PERIOD: Duration = Duration::from_secs(1);

    fn sample(&mut self, bus: &mut dyn Bus) -> Result<Gas, DriverError> {
        let air = lock(&self.latest).air;
        let (temp_c, rh_pct) = air.map_or((25.0, 50.0), |a| air::offset(a.temperature_c, a.humidity_pct, -self.offset_c));
        let raw = self.sgp.measure(bus, rh_pct, temp_c)?;
        let index = self.voc.process(raw.into());
        self.tick(unix_ms());
        // The algorithm returns 0 while warming up; a real index is >= 1.
        Ok(Gas { voc_index: u16::try_from(index).ok().filter(|&i| i > 0), raw })
    }

    fn publish(&self, reading: &mut Reading, gas: Option<Gas>) {
        reading.gas = gas;
    }

    /// A restart then resumes from the baseline at the stop, not one up to
    /// 10 minutes older.
    fn stop(&mut self) {
        self.save(unix_ms());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Model, round, temp_dir};

    #[test]
    fn repeated_errors_are_logged_at_most_every_10_minutes() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut log = ErrorLog::default();
        let e = || "humidity reads 0 %".to_string();
        assert_eq!(log.line(e(), at(0)), Some(e()));
        assert_eq!(log.line(e(), at(1)), None);
        assert_eq!(log.line(e(), at(599)), None);
        assert_eq!(log.line(e(), at(600)), Some("humidity reads 0 % (repeated 2 times since last logged)".into()));
        assert_eq!(log.line(e(), at(601)), None);
        assert_eq!(log.line("CRC mismatch".into(), at(602)), Some("CRC mismatch".into()));
        assert_eq!(log.line(e(), at(603)), Some(e()));
    }

    #[test]
    fn readings_align_to_the_wall_clock() {
        assert_eq!(until_aligned(65_000, 30_000), 25_000);
        assert_eq!(until_aligned(60_000, 30_000), 30_000);
        assert_eq!(until_aligned(89_999, 30_000), 1);
    }

    #[test]
    fn missed_slots_are_dropped_keeping_the_phase() {
        let t0 = Instant::now();
        let s = Duration::from_secs(1);
        let ms = Duration::from_millis;
        assert_eq!(next_slot(t0, s, t0 + ms(300)), t0 + s);
        assert_eq!(next_slot(t0, s, t0 + ms(1000)), t0 + 2 * s);
        assert_eq!(next_slot(t0, s, t0 + ms(3500)), t0 + 4 * s);
    }

    fn air_chip(offset_c: f64, altitude_m: Option<f64>) -> (AirChip, Model) {
        let mut bus = crate::bme280::tests::live(Model::default());
        let bme = Bme280::new(&mut bus).unwrap();
        (AirChip { bme, offset_c, altitude_m }, bus)
    }

    #[test]
    fn air_without_an_offset_is_the_measurement() {
        let (mut chip, mut bus) = air_chip(0.0, None);
        let a = chip.sample(&mut bus).unwrap();
        assert_eq!([a.temperature_c, a.humidity_pct, a.pressure_hpa].map(|x| round(x, 2)), [51.75, 9.87, 1002.34]);
        assert_eq!(a.sea_level_hpa, None);
    }

    #[test]
    fn air_with_an_offset_keeps_the_measured_dew_point() {
        let (mut chip, mut bus) = air_chip(-15.0, Some(350.0));
        let a = chip.sample(&mut bus).unwrap();
        // 51.75 °C at 9.87 % has an 11.21 °C dew point, which is 21.50 % at
        // 36.75 °C. Sea level at 350 m uses 36.75 °C. Worked by hand from
        // the measurement rounded to 0.01, hence the tolerance.
        let near = |got: f64, want: f64| assert!((got - want).abs() < 0.05, "{got} vs {want}");
        near(a.temperature_c, 36.75);
        near(a.dew_point_c, 11.21);
        near(a.humidity_pct, 21.50);
        near(a.sea_level_hpa.unwrap(), 1041.63);
    }

    fn gas_chip(air: Option<Air>, offset_c: f64) -> (GasChip, Model) {
        let mut bus = Model::default().block(0x59, 0x00, &[&[0xD4, 0x00, 0xC6], &[0x7B, 0x63, 0x48]]);
        let sgp = Sgp40::new(&mut bus).unwrap();
        let latest = Shared::new(Mutex::new(Reading { air, ..Reading::default() }));
        (GasChip::new(sgp, latest, offset_c, None, 0), bus)
    }

    #[test]
    fn gas_compensates_for_the_measured_air_whatever_the_offset() {
        // 9.87 % and 51.75 °C, as the SGP40's humidity and temperature ticks.
        let measured = "wbk 59 26 [0f, 19, 44, d5, 8d, 87, 0f]".to_string();
        for offset_c in [0.0, -15.0] {
            let (temperature_c, humidity_pct) = air::offset(51.75, 9.87, offset_c);
            let air = Air { temperature_c, humidity_pct, pressure_hpa: 1000.0, dew_point_c: 11.21, sea_level_hpa: None };
            let (mut chip, mut bus) = gas_chip(Some(air), offset_c);
            assert_eq!(chip.sample(&mut bus), Ok(Gas { voc_index: None, raw: 31587 }));
            assert_eq!(bus.ops.last().unwrap(), "rbk 59 00 3");
            assert!(bus.ops.contains(&measured), "offset {offset_c}: {:?}", bus.ops);
        }
    }

    #[test]
    fn gas_uses_25_c_and_50_percent_before_the_first_air_sample() {
        let (mut chip, mut bus) = gas_chip(None, 0.0);
        chip.sample(&mut bus).unwrap();
        assert!(bus.ops.contains(&"wbk 59 26 [0f, 80, 00, a2, 66, 66, 93]".to_string()), "{:?}", bus.ops);
    }

    #[test]
    fn voc_index_is_none_until_the_warm_up_ends() {
        let (mut chip, mut bus) = gas_chip(None, 0.0);
        for _ in 0..46 {
            assert_eq!(chip.sample(&mut bus).unwrap().voc_index, None);
        }
        // First real sample: the fresh estimator puts 31587 raw far above its
        // mean, the sigmoid gives 0, and the 0.5 floor rounds up to 1.
        assert_eq!(chip.sample(&mut bus).unwrap().voc_index, Some(1));
    }

    fn sgp() -> Sgp40 {
        let mut bus = Model::default().block(0x59, 0x00, &[&[0xD4, 0x00, 0xC6]]);
        Sgp40::new(&mut bus).unwrap()
    }

    #[test]
    fn voc_state_is_saved_every_10_minutes_after_3_hours() {
        let dir = temp_dir("voc-learn");
        let mut chip = GasChip::new(sgp(), Shared::default(), 0.0, Some(dir.clone()), 0);
        let saved = || crate::state::load_voc(&dir, 0);
        for _ in 0..VOC_LEARNING_S - 1 {
            chip.tick(0);
        }
        assert_eq!(saved(), None);
        chip.tick(0);
        assert_eq!(saved(), Some(chip.voc.state()));
        std::fs::remove_file(dir.join("voc.state")).unwrap();
        for _ in 0..VOC_SAVE_EVERY_S - 1 {
            chip.tick(0);
        }
        assert_eq!(saved(), None);
        chip.tick(0);
        assert_eq!(saved(), Some(chip.voc.state()));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_restored_voc_state_is_resumed_and_saved_without_relearning() {
        let dir = temp_dir("voc-restore");
        crate::state::save_voc(&dir, 1_000_000, (11102.89, 4151.1953)).unwrap();
        let mut chip = GasChip::new(sgp(), Shared::default(), 0.0, Some(dir.clone()), 1_000_000 + 60_000);
        assert_eq!(chip.voc.state(), (11102.89, 4151.1953));
        // Saved again at the restore: the seeded file alone is stale by now.
        assert_eq!(crate::state::load_voc(&dir, 1_060_000 + 599_999), Some((11102.89, 4151.1953)));
        std::fs::remove_file(dir.join("voc.state")).unwrap();
        for _ in 0..VOC_SAVE_EVERY_S {
            chip.tick(2_000_000);
        }
        assert_eq!(crate::state::load_voc(&dir, 2_000_000), Some((11102.89, 4151.1953)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stopping_saves_a_trusted_voc_state_between_periodic_saves() {
        let dir = temp_dir("voc-stop");
        let state = (11102.89, 4151.1953);
        crate::state::save_voc(&dir, 1_000_000, state).unwrap();
        let mut chip = GasChip::new(sgp(), Shared::default(), 0.0, Some(dir.clone()), 1_000_000 + 60_000);
        chip.tick(1_000_000 + 61_000);
        // The 1970 save is decades stale now; only a save at the stop loads.
        assert_eq!(crate::state::load_voc(&dir, unix_ms()), None);
        chip.stop();
        assert_eq!(crate::state::load_voc(&dir, unix_ms()), Some(state));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stopping_the_hat_saves_the_voc_state() {
        let dir = temp_dir("voc-hat-stop");
        let nine_minutes_ago = unix_ms() - 9 * 60_000;
        crate::state::save_voc(&dir, nine_minutes_ago, (11102.89, 4151.1953)).unwrap();
        let bus = Model::default().block(0x59, 0x00, &[&[0xD4, 0x00, 0xC6], &[0x7B, 0x63, 0x48]]);
        let hat = Hat::start_on(bus, Config { state_dir: Some(dir.clone()), ..Config::default() }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while hat.latest().gas.is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        // Removes the save made at the restore, so only the stop can write it.
        std::fs::remove_file(dir.join("voc.state")).unwrap();
        hat.stop();
        // The baseline does not move during the 45 s warm-up.
        assert_eq!(crate::state::load_voc(&dir, unix_ms()), Some((11102.89, 4151.1953)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn stopping_before_3_hours_of_learning_saves_nothing() {
        let dir = temp_dir("voc-stop-early");
        let mut chip = GasChip::new(sgp(), Shared::default(), 0.0, Some(dir.clone()), 0);
        chip.tick(0);
        chip.stop();
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_stale_voc_state_is_ignored() {
        let dir = temp_dir("voc-stale");
        crate::state::save_voc(&dir, 1_000_000, (11102.89, 4151.1953)).unwrap();
        let chip = GasChip::new(sgp(), Shared::default(), 0.0, Some(dir.clone()), 1_000_000 + 600_000);
        assert_eq!(chip.voc.state(), VocIndex::default().state());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_hat_publishes_and_stops_promptly() {
        let bus = crate::bme280::tests::live(Model::default());
        let hat = Hat::start_on(bus, Config::default()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while hat.latest().air.is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let r = hat.latest();
        assert_eq!(r.air.map(|a| round(a.temperature_c, 2)), Some(51.75));
        assert_eq!((r.light, r.uv, r.gas), (None, None, None));
        let start = Instant::now();
        hat.stop();
        assert!(start.elapsed() < Duration::from_millis(500), "stop took {:?}", start.elapsed());
    }

    #[test]
    fn every_yields_on_the_wall_clock_slot() {
        let hat = Hat::start_on(crate::bme280::tests::live(Model::default()), Config::default()).unwrap();
        let r = hat.every(Duration::from_secs(1)).next().unwrap();
        assert!(r.unix_ms % 1000 < 500, "snapshot at {} ms past the second", r.unix_ms % 1000);
    }

    #[test]
    fn a_stop_handle_ends_every() {
        let hat = Hat::start_on(crate::bme280::tests::live(Model::default()), Config::default()).unwrap();
        let handle = hat.stop_handle();
        let start = Instant::now();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            handle.stop();
        });
        assert_eq!(hat.every(Duration::from_secs(3600)).next(), None);
        assert!(start.elapsed() < Duration::from_millis(500), "every ended after {:?}", start.elapsed());
        hat.stop();
    }
}
