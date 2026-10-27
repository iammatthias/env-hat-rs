# Accuracy

For each value `Hat` reports, this page gives the method, the oracle, and
the test that checks the code against the oracle. An oracle is an answer
worked out apart from the code: a datasheet example, a table, or a hand
calculation. Where [ADR 0002: the `Hat` API](adr/0002-hat-api.md) disagrees, this
page is right.

| Value | Method | Oracle | Test |
|---|---|---|---|
| Temperature and humidity | The double-precision compensation in the BME280 datasheet, section 8.1, reported as measured. The Pi warms the board, so the temperature reads above the room. `Hat` adds `Config::temperature_offset_c`, 0 by default, to the temperature, then gives the humidity at the new temperature with the measured dew point. | The BMP280 worked example for temperature. Psychrometric tables for the offset: 25 °C at 50% has a dew point of 13.9 °C, and a 13.85 °C dew point at 20 °C is 67.7%. | `bme280::tests`, `air::tests::an_offset_moves_the_temperature_at_a_constant_dew_point` |
| Dew point | The Magnus formula with the Sonntag 1990 coefficients, 17.62 and 243.12 °C, from the measured temperature and humidity. | The psychrometric tables in the temperature row. | `air::tests::dew_point_matches_the_psychrometric_table` |
| Pressure | The double-precision compensation in the BME280 datasheet, section 8.1, with temperature first so `t_fine` is current. | The worked example in the BMP280 datasheet, which uses the same compensation: 25.08 °C and 100,653.27 Pa. | `bme280::tests::compensation_matches_the_bosch_example` |
| Sea-level pressure | The hypsometric formula with a standard lapse rate: p₀ = p · (1 − 0.0065 h / (T + 0.0065 h + 273.15))^−5.257, with T the reported temperature. | 1,000 hPa at 100 m and 15 °C gives 1,011.92 hPa by hand. | `air::tests::sea_level_pressure_at_100_m` |
| Light | TSL2591 lux from both channels, with auto-gain. The formula (lux coefficient 408) and the gains are Adafruit's. The ams datasheet has no lux equation. Uncalibrated. | Hand-worked values of that formula. | `tsl2591::tests` |
| UV index | UV counts at gain 18 and 20-bit resolution over 1,400 counts per UVI. | The [LTR-390UV-01 datasheet V1.2](https://files.waveshare.com/upload/a/ac/1_LTR-390UV_Final_DS_V1_2.pdf), table 4.5 and figure 4.4. | `ltr390::tests::one_uv_index_is_1400_counts` |
| Ambient lux | The LTR390 datasheet formula 0.6 · ALS / (gain · int), at gain 3 and int 4 (20-bit). | 40,000 counts give 2,000 lux. | `ltr390::tests::the_first_read_waits_for_a_uv_conversion` |
| VOC Index | The float implementation of Sensirion's gas-index algorithm (first released in 3.2.1), ported operation for operation. The baseline persists in `voc.state`. | Sensirion's reference C over 14 h, and over a 4 h run whose state restores into a fresh instance that matches the C for 2 more hours. | `tests/gas_index.rs` |
| Acceleration, rotation, field | Signed 16-bit decode. ±2 g at 16,384 LSB/g, ±1,000 dps at 32.8 LSB/dps, 0.15 µT/LSB. The gyroscope offset is the mean of 32 samples at start. | Hand-decoded bytes. | `icm20948::tests` |
| Magnetometer read | One 23-byte burst returns the motion and the AK09916's ST1 to ST2. An overflow clears only the field and the heading. See [How a stalled magnetometer is detected](#how-a-stalled-magnetometer-is-detected). | The live board: WIA reads 0x48 0x09, and the field magnitude is plausible. A register model of the chip. | `icm20948::tests`, `examples/soak.rs` |
| Roll, pitch | From gravity: roll = atan2(ay, az), pitch = atan2(−ax, hypot(ay, az)). | Hand-worked vectors at level, 90°, and 45°. | `orientation::tests::roll_and_pitch_from_gravity` |
| Heading | Tilt-compensated: east = field × up, north = up × east, heading = atan2(east · x, north · x). The hard-iron offsets come from `Config::hard_iron_ut`. | Hand-worked vectors: the four level directions, and level pairs rotated 30° about x and 45° about y, which keep their heading. | `orientation::tests` |

The UV row uses 1,400 counts per UVI. V1.1 of the same datasheet gave
2,300, which is why Adafruit, ESPHome, and the Linux driver divide by
2,300.

## How a stalled magnetometer is detected

When the ICM20948's I2C master stops, its copy of the field keeps its last
bytes, and a frozen field looks like a still one. `Hat` reports `mag_ut`
as `None` when either of these is true:

- `USER_CTRL` has `I2C_MST_EN` clear.
- HX, HY, and HZ repeat byte for byte over four samples.

On the live board, each axis repeats between two samples about 5% of the
time (σ ≈ 7 LSB over 1,458 pairs). Four identical samples by chance happen
about once in 10⁴ years at 1 Hz.

While the field is stalled, `Hat` restarts the I2C master and the AK09916,
at most once a minute. A magnetometer that fails to start leaves the
acceleration and rotation readable, and `Hat` retries it the same way.

Before it resets the chip at start, `Hat` stops any master that a previous
process left polling. A reset during a transaction once left the AK09916
unresponsive for about 20 minutes.

These checks ran on the live board:

- Clearing `I2C_MST_EN` from another process gave one `None` sample, then
  a restart and a current field.
- Twelve service restarts in a row each started the magnetometer.

The register model in `icm20948::tests` covers a stopped master, a
powered-down AK09916, a restart that keeps failing, one that fails with
bank 3 selected, and a start over a polling master.

## What is not verified

- The compass calibration. Measuring hard-iron offsets needs someone to
  turn the board through every orientation, so heading has not been
  checked on the device. Uncalibrated, the live board reads a field of
  about 72 µT, which still includes the hard-iron offset of the board and
  the Pi. To measure the offsets, see
  [how to measure the hard-iron offsets](compass.md).
- The temperature offset for any one installation. It depends on the
  case, the airflow, and the Pi's load, so the crate ships none. Measure
  it against a reference thermometer next to the board. For the steps, see
  [how to set the temperature offset](api.md#config).
- The VOC baseline's age after a long power-off. The Pi has no real-time
  clock. Until NTP syncs, systemd-timesyncd sets the clock to about the
  last shutdown time, so an old baseline can look minutes old and be
  restored. The algorithm then adapts from that baseline, with its 12-hour
  time constant for the mean, instead of learning from scratch.
