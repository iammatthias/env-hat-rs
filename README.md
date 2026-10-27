# env-hat

Rust drivers for the [Waveshare Environment Sensor HAT][hat] on a Raspberry
Pi. `Hat::start` runs one sampler thread per chip on `/dev/i2c-1`, each
at its chip's natural rate, and keeps the newest value of every chip in
one typed `Reading`. `Reading::values()` names each value for any
time-series database. The drivers depend on `libc` and nothing else.

| Chip | Address | Measures |
|---|---|---|
| BME280 | 0x76 | temperature, humidity, pressure |
| TSL2591 (the wiki calls it TSL25911FN) | 0x29 | ambient light, visible and infrared |
| LTR390 | 0x53 | UV, ambient light |
| SGP40 | 0x59 | VOC raw signal, VOC Index |
| ICM20948, with the AK09916 magnetometer in the same package | 0x68 | acceleration, rotation rate, magnetic field |

## Requirements

- A Raspberry Pi with the 40-pin header, running 64-bit Linux. The crate is
  tested on a Pi Zero 2 W with Raspberry Pi OS Lite (Debian trixie, arm64).
  It uses Linux i2c-dev, so it does not build for microcontrollers.
- The HAT seated on the header. All five chips share I2C bus 1, on the
  header's SDA and SCL pins.
- I2C turned on. Run `sudo raspi-config nonint do_i2c 0`, then reboot.
  `/dev/i2c-1` must exist.
- Read and write access to `/dev/i2c-1`. Run `sudo usermod -aG i2c $USER`,
  then log in again.
- Rust 1.85 or later.

To check the board, run `i2cdetect -y 1` from the `i2c-tools` package. It
shows 29, 53, 59, 68, and 76, the addresses in the chip table. It
also shows 28, because the TSL2591 answers at 0x28 as well as 0x29. The
AK09916 at 0x0C sits behind the ICM20948 and does not appear. The wiki's
spec table lists the SGP40 at 0x60, but its sensor section and the chip
use 0x59.

## Install

The crate is not on crates.io. Add it as a git dependency:

```toml
[dependencies]
env-hat = { git = "https://github.com/iammatthias/env-hat-rs", branch = "main" }
```

To build for the Pi on a Mac or an x86 machine, see
[how to build and run on the live board](docs/development.md#test-on-the-live-board).

## Read the sensors

This program prints the air every 10 s, at :00, :10, :20, :30, :40, and :50
past each minute. It also prints the VOC Index after the index is ready.

```rust,no_run
use std::time::Duration;

use env_hat::{Config, Hat};

fn main() -> Result<(), env_hat::Error> {
    let hat = Hat::start(Config::default())?;
    for reading in hat.every(Duration::from_secs(10)) {
        if let Some(air) = reading.air {
            println!("{:.1} °C  {:.0} %RH  {:.1} hPa", air.temperature_c, air.humidity_pct, air.pressure_hpa);
        }
        if let Some(voc) = reading.gas.and_then(|g| g.voc_index) {
            println!("VOC Index {voc}");
        }
    }
    Ok(())
}
```

Run it with `cargo run --release`. The output looks like this:

```text
37.9 °C  18 %RH  1003.8 hPa
37.8 °C  18 %RH  1003.8 hPa
...
37.8 °C  18 %RH  1003.8 hPa
VOC Index 1
37.8 °C  18 %RH  1003.8 hPa
VOC Index 34
```

The first line comes at the next 10 s mark. The VOC line starts about a
minute in, after the algorithm's 46 warm-up samples. It starts low and
settles near 100, the sensor's recent average air.

The temperature and humidity are what the BME280 measures on the board.
The Pi's processor warms the board, so the temperature reads above the
room. How far depends on the case, the airflow, and the Pi's load. To
compensate, set `Config::temperature_offset_c` from a reference
thermometer. For the steps, see
[how to set the temperature offset](docs/api.md#config).

To end the loop from another thread, such as a SIGTERM handler, take
`hat.stop_handle()` before the loop and call `stop()` on it there. The
loop ends, and `hat.stop()` then waits for the samplers.

## Troubleshooting

When the example fails, Rust prints the error's debug form, such as
`Error: Bus(BusError(13))`. The number is the OS error that the messages
in this list quote, so 13 is `Permission denied (os error 13)`.

- `opening the i2c bus: Permission denied (os error 13)`. Your user is not
  in the `i2c` group. Add it with `sudo usermod -aG i2c $USER`, then log
  in again.
- `opening the i2c bus: No such file or directory (os error 2)`. I2C is
  off. Turn it on with `raspi-config` and reboot.
- `env-hat: <chip>: not started: <error>` on stderr. That chip did not
  answer, and its group stays `None`. Reseat the HAT, then run
  `i2cdetect -y 1`. If another program uses the bus, stop that program first.
- Every group is `None`. The chips have not sampled yet. Read through
  `every()`, or wait a few seconds after `start()`.

## Documentation

- [The board](docs/hardware.md) lists the chips and how `Hat` configures
  each one.
- [The API reference](docs/api.md) describes `Hat`, `Config`, `Bus`,
  `Reading`, and `VocIndex`.
- [Accuracy](docs/accuracy.md) describes how each value is computed, and the
  test that checks it.
- [How to measure the compass's hard-iron offsets](docs/compass.md) gives
  the steps that heading needs.
- [Development](docs/development.md) covers the tests, the local CI, adding
  a dependency, and testing on the live board.
- [How to record readings](docs/recording.md) lists the name and unit for
  each value, with examples for CSV, InfluxDB, and SQL.
- [Architecture decisions](docs/adr/README.md) explain why the API has the
  shape it has.
- [The release checklist](docs/release.md) is for maintainers. It lists
  what is left before the crate is published.

## License

`env-hat` is under the [MIT license](LICENSE). `crates/env-hat/src/gas_index.rs`
ports the floating-point implementation of Sensirion's
gas-index-algorithm, first released in 3.2.1, under Sensirion's BSD
3-Clause license. Its text is in that file and in
`crates/env-hat/tests/gas_index_ref/LICENSE`.

[hat]: https://www.waveshare.com/wiki/Environment_Sensor_HAT
