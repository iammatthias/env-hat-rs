# API reference

The `env_hat` crate reads the HAT through one type, `Hat`. This page
describes `Hat`, `StopHandle`, `Config`, `Bus`, `Reading`, and `VocIndex`.
For the reasons behind this shape, see
[ADR 0002: the `Hat` API](adr/0002-hat-api.md).

## `Hat`

| Item | Behavior |
|---|---|
| `Hat::start(Config) -> Result<Hat, Error>` | Opens `/dev/i2c-{bus}` and starts one sampler thread per chip. Fails only if it cannot open the bus device or spawn a thread. |
| `Hat::start_on(bus, Config)` | `start` on any `Bus` implementation, for tests and bus instrumentation. `crates/env-hat/examples/soak.rs` uses it. |
| `latest(&self) -> Reading` | Copies the newest value of every chip. Never touches the bus. Sets `unix_ms` to the current time. |
| `every(&self, Duration) -> impl Iterator<Item = Reading>` | Yields `latest()` at every multiple of the period on the wall clock. For a 30 s period, that is at :00 and :30 past each minute. A slow consumer skips slots instead of receiving a burst. The iterator ends when a `StopHandle` stops the `Hat`. |
| `stop_handle(&self) -> StopHandle` | A handle that stops the `Hat` from another thread. |
| `stop(self)` | Stops the samplers and waits for any sample in progress. Dropping the `Hat` does the same. If `state_dir` is set and the VOC baseline can be trusted, the SGP40 sampler then saves it (see [The VOC baseline file](#the-voc-baseline-file)). The longest wait is an LTR390 sample that needs both of its conversion waits, about 3 s. A chip still starting adds its start-up time. The ICM20948's is about 0.6 s. |

## `StopHandle`

`StopHandle` is `Clone + Send`. Its `stop(&self)` asks every sampler to
stop after its current sample and ends `every()`. It returns at once. Call
`Hat::stop` afterwards to wait for the samplers.

## Errors and logging

`Error` is why `start` failed: `Error::Bus(BusError)` when `start` cannot
open the bus device, or `Error::Thread(io::Error)` when it cannot spawn a
thread.

After `start`, the samplers report on stderr. Every line has the form
`env-hat: <chip>: <message>`.

| Message | Meaning |
|---|---|
| `not started: <error>` | The chip did not start. Its group stays `None`. |
| `read error: <error>` | One sample failed. The group is `None` until the next good sample. |
| `magnetometer not started: <error>` | The AK09916 did not start. The sampler still reads acceleration and rotation. The sampler retries once a minute. |
| `magnetometer stalled; restarting it at most once a minute` | The field stopped updating. `mag_ut` is `None` until it recovers. |
| `magnetometer recovered` | The field updates again. |
| `saving the VOC state in <dir>: <error>` | The SGP40 sampler could not write the VOC baseline file. |

The same read error from one chip prints at most once every 10 minutes,
followed by how many repeats were suppressed. A different error prints
immediately.

## Sampling

Each chip has its own thread and period. Each thread publishes its group
into one shared `Reading`, and it is the only writer of that group.

| Thread | Period | Publishes |
|---|---|---|
| `bme280` | 2 s | `air` |
| `tsl2591` | 5 s | `light` |
| `ltr390` | 10 s | `uv` |
| `sgp40` | 1 s | `gas` |
| `icm20948` | 1 s | `motion` |

The schedules are fixed-rate. When a sample overruns its slot, the sampler
drops the missed slots, and later samples stay on the original schedule. The
threads share the bus through a lock taken for one transaction at a time.
Conversion waits and auto-ranging happen outside the lock, so a slow chip
never delays another. Each chip's first sample waits for its first
conversion.

## `Config`

| Field | Default | Meaning |
|---|---|---|
| `bus: u32` | `1` | The `N` in `/dev/i2c-N`. |
| `state_dir: Option<PathBuf>` | `None` | The directory for the VOC Index baseline, created when first needed. Without it, VOC learning restarts with every start. |
| `hard_iron_ut: Option<[f64; 3]>` | `None` | The magnetometer's hard-iron offsets in µT. To measure them, see [how to measure the hard-iron offsets](compass.md). Without them, `heading_deg` stays `None`. |
| `temperature_offset_c: f64` | `0.0` | Added to the BME280's temperature, in °C. With an offset, `Hat` gives `humidity_pct` at the new temperature with the measured dew point. |
| `altitude_m: Option<f64>` | `None` | When set, `Hat` reports `Air::sea_level_hpa`. |

By default `Air` reports what the BME280 measured. The Pi's processor
warms the board, so the temperature reads above the room. To compensate,
put a reference thermometer next to the board, wait until both readings
are steady, and set `temperature_offset_c` to the reference minus
`temperature_c`. The Pi's heat changes with its load and its case, so an
offset measured at one load is only an approximation at another.

### The VOC baseline file

`state_dir/voc.state` holds the VOC Index baseline. It is the save time in
Unix milliseconds, then the baseline's mean and standard deviation,
separated by spaces. Don't edit this file.

```text
1761400000000 11102.89 4151.1953
```

`Hat` saves the baseline only when it can be trusted, which means the
algorithm has learned for 3 hours or `Hat` restored it. It then saves right
away, every 10 minutes, and when the `Hat` stops. At start, `Hat` restores
the file if it is less than 10 minutes old and its numbers are plausible.
A plausible mean is 0 to 65,535, and a plausible standard deviation is
above 0 and at most 65,535.

## `Bus`

`Bus` is the four register transactions the drivers need, plus a default
`sleep`. `I2cDev` implements it on Linux. Implement it to test or
instrument the drivers, and pass it to `Hat::start_on`.

| Method | Transaction |
|---|---|
| `read_byte(addr, reg) -> Result<u8, BusError>` | One register. |
| `write_byte(addr, reg, value)` | One register. |
| `read_block(addr, reg, buf: &mut [u8])` | `buf.len()` registers from `reg`, 1 to 32. |
| `write_block(addr, reg, data: &[u8])` | `reg`, then 1 to 32 bytes of `data`. |
| `sleep(Duration)` | A driver's wait for a conversion. The default sleeps the thread. `Hat` does not forward it to your bus, so an override has no effect under a `Hat`. |

`I2cDev::open(bus)` opens `/dev/i2c-{bus}`. `BusError` holds the errno.

## `Reading`

`unix_ms` is the snapshot time. Each chip has one optional group. A group
is `None` while its chip is absent, failing, or has not produced a first
sample.

| Group | Fields |
|---|---|
| `air: Option<Air>` | `temperature_c` and `humidity_pct` (as measured, plus any `temperature_offset_c`), `pressure_hpa`, `dew_point_c`, `sea_level_hpa: Option<f64>` |
| `light: Option<Light>` | `lux` (Adafruit's formula, uncalibrated), `full: u16`, `ir: u16` (channel counts), `gain` (the nominal 1, 25, 428, or 9,876) |
| `uv: Option<Uv>` | `index`, `counts: u32`, `ambient_lux: Option<f64>` (`None` if its conversion timed out) |
| `gas: Option<Gas>` | `voc_index: Option<u16>` (1 to 500, `None` for the first 46 samples while the algorithm returns 0), `raw: u16` |
| `motion: Option<Motion>` | `roll_deg`, `pitch_deg`, `heading_deg: Option<f64>`, `accel_g: [f64; 3]`, `gyro_dps: [f64; 3]`, `mag_ut: Option<[f64; 3]>`, `temperature_c` (the die, by the untrimmed sensitivity) |

`mag_ut` is `None` while the field exceeds the AK09916's ±4,912 µT range,
while the magnetometer is stalled, and before it has started.
`heading_deg` is `None` without `Config::hard_iron_ut` or without
`mag_ut`.

`Motion` uses the board frame: x and y along the board, z out of its top
face. Lying flat, `accel_g` reads about `[0, 0, 1]`. `heading_deg` is the
direction of the board's x axis, clockwise from magnetic north, 0 to 360.

The rustdoc on each field gives its unit.

`Reading::values()` returns every present value with its
`schema::Field`, a fixed `name` and `unit` to record it under. For the
full schema and examples, see [how to record readings](recording.md).

## `gas_index::VocIndex`

Sensirion's VOC Index algorithm on its own, for an SGP40 you read
yourself. `Hat` uses it for `gas.voc_index`.

| Item | Behavior |
|---|---|
| `VocIndex::default()` | A fresh algorithm. |
| `process(&mut self, raw: i32) -> i32` | Takes one raw SGP40 sample. Call it once a second. Returns 0 for the first 46 samples, then 1 to 500. |
| `state(&self) -> (f32, f32)` | The learned baseline, as its mean and standard deviation. Sensirion says it means something only after 3 hours of operation. |
| `set_state(&mut self, mean, std)` | Restores a baseline on a fresh instance. Sensirion says not to restore one after more than 10 minutes off. |
