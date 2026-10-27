# ADR 0002: the `Hat` API, sampling, and accuracy

Status: accepted 2026-10-25, amended 2026-10-26. The following sketch
predates the code. Where they differ, [Implementation reconciliation](#implementation-reconciliation),
[API](../api.md) and [Accuracy](../accuracy.md) describe the code.

## Problem

Today a caller juggles a `Profile`, a `CompCfg`, a log callback, and a
`Sensors` poller. It has to call `tick()` once a second and `read()` every
interval, and it gets back a flat struct of `Option<f64>` with
Python-compatibility types inside. Each chip has a natural rate. The SGP40
needs 1 Hz for its VOC Index, the BME280 is cheap at any rate, and the
LTR390's mode switch takes about a second. One slow chip can't be allowed
to delay the others. Several readings today are less accurate than the
hardware allows. Humidity is reported at the hot board, not in the room.
The IMU's sign handling is off by one LSB. Heading comes from a filter
designed for 20 Hz that runs once per 30 s. And VOC learning is lost on
every restart.

## Usage (caller's view)

This was the proposed usage. For the API as built, see [API](../api.md).

```rust,ignore
use env_hat::Hat;

// Starts one sampler thread per chip present on /dev/i2c-1.
let hat = Hat::start(Default::default())?;

// A snapshot of the newest values. It never touches the bus.
let now = hat.latest();
if let Some(air) = &now.air {
    println!("{:.1} °C  {:.0} %RH  {:.1} hPa", air.temperature_c, air.humidity_pct, air.pressure_hpa);
}
if let Some(voc) = now.gas.as_ref().and_then(|g| g.voc_index) {
    println!("VOC index {voc}");
}

// A reading every 30 s, aligned to the clock, for recording.
for reading in hat.every(std::time::Duration::from_secs(30)) {
    store.append(reading.unix_ms, &reading.fields())?;
}
```

A recorder is that last loop plus a store. A one-shot script calls
`Hat::start(..)?.latest()` after a warm-up. A planned `env-hat` command
with `record` and `read` subcommands would wrap the same two calls. It
does not exist yet.

## Shape

| Part | Proposal |
|---|---|
| `Hat` | `start(Config) -> Result<Hat, Error>`, `latest() -> Reading`, `every(Duration) -> impl Iterator<Item = Reading>`, and `stop()`, also on drop. It hides the bus sharing, the threads, the per-chip rates, the compensation, and the VOC state file. |
| `Config` | Every field has a default. `bus: u32` (1). `state_dir: Option<PathBuf>`, where VOC learning and the compass calibration persist. `self_heating: SelfHeating { factor: 1.22, tau_s: 300.0 }`, or `None` to report the board's own values. `altitude_m: Option<f64>`, which adds sea-level pressure. |
| `Reading` | `unix_ms` plus one optional typed group per chip: `air`, `light`, `uv`, `gas`, `motion`, and `cpu_temp_c`. A group is `None` while its chip is absent, failing, or hasn't produced a first sample. `Reading::fields()` flattens it into the existing store field names, so history stays continuous. |
| Sampling | One thread per present chip, each on its own schedule: BME280 2 s, TSL2591 5 s, LTR390 10 s, SGP40 1 s, ICM20948 1 s, and the CPU temperature 2 s. |
| Vendor fidelity | The vendor-faithful poller moves to `env_hat::vendor`, with the replay traces as its tests. `Hat` always runs the datasheet configuration, so `Profile` leaves the public API. |

Each thread owns its driver and writes its group into one shared
`Mutex<Reading>`. That is the only shared state, and each field group has
one writer. The bus is a `SharedBus`, an `Arc<Mutex<I2cDev>>` locked per
transaction, with the cache of the current target address inside the lock. Sleeps
happen outside the lock, so a 1.35 s TSL auto-range never delays an SGP40
sample. Chips on different addresses can interleave safely, because
multi-transaction sequences such as the ICM bank selects touch only their
own chip. Schedules are fixed-rate with dropped slots.

## Accuracy decisions (each with its oracle)

| Quantity | Method | Oracle |
|---|---|---|
| Room temperature | Thermal divider from the board temperature and a low-passed SoC temperature (the existing model, factor 1.22) | Unchanged from today. Replay-proven. |
| Room humidity | Dew point is conserved: Magnus dew point from the board's temperature and humidity, then the relative humidity at the room temperature. Its accuracy is bounded by the room-temperature estimate. | Psychrometric known answers: 25 °C at 50% gives a dew point of 13.9 °C. A 13.85 °C dew point at 20 °C gives about 67.7% (NOAA calculator, within 0.3%). |
| Pressure | Bosch compensation (`Full`). Sea level by the hypsometric formula when `altitude_m` is set. | Bosch reference values, and a hand-worked sea-level case. |
| Light | TSL2591 auto-gain | Existing `gain_step` table |
| UV index, ambient lux | LTR390 datasheet formulas | Existing tests |
| VOC Index | Sensirion algorithm. Its mean and standard deviation are saved every 10 min and restored if younger than 10 min (Sensirion's guidance). | 14 h C-reference test, plus a round trip through the get-state and set-state functions against the C reference. |
| Accel, gyro, mag | True two's complement (`i16`) decode. Accel ±2 g at 16,384 LSB/g, gyro ±1,000 dps at 32.8 LSB/dps, mag 0.15 µT/LSB. The gyro offset is averaged at start. | Hand-decoded byte vectors |
| Magnetometer read | The ICM's I2C master reads AK09916 ST1 to ST2 continuously into EXT_SENS_DATA. One burst read of bank 0 returns accel, gyro, temperature, and mag. | Live board (WIA and plausible field magnitude) |
| Roll, pitch | From gravity: `roll = atan2(ay, az)`, `pitch = atan2(-ax, hypot(ay, az))` | Hand-worked vectors (level, 90°, 45°) |
| Heading | Tilt-compensated magnetic heading with hard-iron offsets from `state_dir/compass.json`. `None` until calibrated. | Hand-worked vectors. Calibration needs someone rotating the board, so it is untested on the device |

## Tradeoffs accepted

The design accepts these costs:

- Roughly six threads, each with a small stack, to get per-chip rates and
  isolation. Memory is under 1 MB in total.
- `latest()` mixes samples up to one chip period old, in exchange for
  never blocking on the bus.
- `Profile` leaves the public API. The vendor port stays
  available for reference and tests.
- Heading stays `None` until someone calibrates the compass, rather than
  reporting a confident but wrong angle.

## Alternatives considered

- **A single scheduler thread with non-blocking chip state machines.** It
  has no locks, but each driver becomes a hand-written state machine around
  its sleeps (TSL settle, LTR mode switch, SGP measure). That is more code,
  and every driver gets harder to read.
- **Keeping `Sensors` and adding sampling beside it.** That gives two ways
  to read the HAT, and callers copy whichever they find first.
- **Implementing `embedded-hal`.** The drivers run only on Linux i2c-dev.
  A four-method `Bus` trait keeps the dependency list at `libc`.

## Implementation reconciliation

The implementation departs from the preceding sketch in the following ways.

| Sketch | Implementation | Accepted in |
|---|---|---|
| `Hat::start` is the only constructor. | `Hat::start_on(bus, Config)` also starts the `Hat` on any `Bus`, for tests and bus instrumentation. | The first `Hat` commit |
| `every()` never ends. | `every()` ends when a `StopHandle` stops the `Hat`, so a recorder can stop on SIGTERM. | The full-pass review |
| `Reading::fields()` and its store names live in the crate. | The store names belong to the application that records them, which keeps them and their history. `env-hat` has no `serde`. Since 2026-10-27 it publishes canonical names through `Reading::values()` and the `schema` module, per [ADR 0003](0003-schema-not-store.md). An application with its own recorded history can keep its names. | The full-pass review, and 2026-10-27 |
| The vendor poller is `env_hat::vendor`. | It became the unpublished `env-hat-vendor` crate, and was deleted after nothing depended on it. | The full-pass review, and 2026-10-27 |
| Six threads, one of them for the CPU temperature. | Five, and no CPU temperature. | The first review round, and 2026-10-27 |
| `state_dir` holds the compass calibration in `compass.json`. | `Config::hard_iron_ut` takes the offsets. The only state file is `voc.state`, three numbers. | The first review round |
| `mag_ut: [f64; 3]`. | `mag_ut: Option<[f64; 3]>`, `None` on an overflow, a stall, or before the magnetometer starts. | The first review round |
| The room temperature oracle is replay-proven. | There is no room estimate. `Air` reports the BME280's measurement, plus an optional `Config::temperature_offset_c` the user measures. A model fitted to one room does not hold in another. | 2026-10-27 |
| Bosch compensation by the `Full` profile. | The datasheet compensation, with no profiles. | The first review round |
| The VOC baseline is saved every 10 minutes and restored if younger than 10 minutes. | It is saved only after the algorithm has learned for 3 hours or after a restore, then right away, every 10 minutes, and at stop. It is restored only when younger than 10 minutes, per Sensirion's guidance. | VOC baseline persistence, and the full-pass review |
| The BME280 runs the datasheet configuration. | `config` is written in sleep mode and read back, because the datasheet says normal-mode writes may be ignored. | The datasheet fact-check |
| The LTR390 uses the datasheet formulas. | UV is measured at gain 18, where the datasheet gives its sensitivity (1,400 counts per UVI), and ambient light at gain 3. | The datasheet fact-check. See [Accuracy](../accuracy.md) for why 2300 counts per UVI was dismissed. |
| The light oracle is the `gain_step` table. | Hand-worked values of Adafruit's lux formula. The formula and gains are community constants, so the lux is uncalibrated. | The first review round |
| The VOC Index is "the Sensirion algorithm". | The float implementation, first released in gas-index-algorithm 3.2.1. | The first review round |
