# The board

The [Waveshare Environment Sensor HAT][hat] carries five sensor chips on
the Raspberry Pi's I2C bus 1 (`/dev/i2c-1`). This page lists each chip,
its address, and how `Hat` configures it. `Hat` uses the datasheet
configuration where the datasheet gives one. This page marks the
community constants it uses instead.

## Chips and addresses

| Chip | Address | What `Hat` reads |
|---|---|---|
| BME280 | 0x76 | temperature, relative humidity, pressure |
| TSL2591 | 0x29 | full-spectrum and infrared channel counts |
| LTR390 | 0x53 | UV counts, then ambient-light counts |
| SGP40 | 0x59 | raw VOC signal, compensated for humidity and temperature |
| ICM20948 | 0x68 | acceleration, rotation rate, die temperature |
| AK09916 | 0x0C on the ICM20948's auxiliary bus | magnetic field |

The wiki calls the TSL2591 the TSL25911FN, and it calls the LTR390 the
LTR390-UV-1. The LTR390's datasheet calls it the LTR-390UV-01.

The AK09916 magnetometer is a separate die in the ICM20948's package, so
it is not on the Pi's bus. The ICM20948 reaches it through its own I2C
master.

`i2cdetect -y 1` shows 28, 29, 53, 59, 68, and 76. The TSL2591
answers at 0x28 as well as at its documented 0x29. `Hat` uses 0x29.

## How `Hat` configures each chip

### BME280

- Oversampling: temperature ×2, pressure ×4, humidity ×1. IIR filter ×4.
  Normal mode, 1,000 ms standby.
- `Hat` writes the configuration in sleep mode: `ctrl_meas` to sleep, then
  `ctrl_hum`, `config`, and `ctrl_meas`. The datasheet says the chip might
  ignore writes to `config` in normal mode.
- `Hat` reads `config` back. If `config` did not take, `Hat` does
  not start the chip.
- After configuring, `Hat` waits 20 ms for the first conversion. The worst
  case at these settings is 18.5 ms.
- Each sample is one burst read of the eight data registers.

### TSL2591

- 200 ms integration. Interrupts off, because `Hat` does not use them.
- Auto-gain steps between the gains 1, 25, 428, and 9,876, at most three
  steps per reading.
- Those gains and the lux formula are Adafruit's community constants. The
  ams datasheet gives typical gains of 24.5, 400, and 9,200 (channel 0) and
  no lux equation. The lux is uncalibrated.

### LTR390

- 20-bit resolution, 400 ms conversion, 500 ms measurement rate.
- `Hat` measures UV at gain 18, where the datasheet gives the UV
  sensitivity. It measures ambient light at gain 3, the gain the datasheet
  specifies its lux accuracy for. `Hat` writes the gain on every mode switch.
- Each reading takes one UV conversion, switches to ambient-light mode for
  one conversion, then switches back.
- The chip starts in standby, so the first reading waits for a UV
  conversion.
- `Hat` accepts any part ID whose upper nibble is 0xB. The low nibble is
  the silicon revision.

### SGP40

- At start, `Hat` runs the datasheet self test (`0x280E`, expecting
  `0xD400`).
- Each sample is `measure_raw` (`0x260F`) with the BME280's temperature and
  humidity, or 25 °C and 50% before the first air sample.
- `Hat` checks the CRC on every response, the self test's included.

### ICM20948

- Accelerometer ±2 g and gyroscope ±1,000 dps, both with the low-pass
  filter on and a sample rate divider of 7. The gyroscope runs at
  1.1 kHz / (1 + 7) = 137.5 Hz (DS-000189 v1.3, section 11.1).
- The I2C master runs at clock code 7, which the datasheet recommends.
- The I2C master polls the magnetometer at 1.1 kHz. After a reset,
  `LP_CONFIG` has `I2C_MST_CYCLE` set, so the master runs duty-cycled at
  the rate `I2C_MST_ODR_CONFIG` sets. That register resets to 0, which is
  1.1 kHz / 2⁰ (sections 8.3 and 11.1). `Hat` leaves both at their reset
  values. A live probe agrees. Against the AK09916's 100 Hz, about 9% of
  master reads are expected to see a new sample, and 7% did.

### AK09916

- Continuous mode at 100 Hz.
- Each master cycle reads registers ST1 to ST2 (9 bytes) into
  `EXT_SLV_SENS_DATA_00`. Reading through ST2 releases the magnetometer's
  data lock.
- An overflow (ST2 HOFL) clears only the field.
- `EXT_SLV_SENS_DATA` keeps its last bytes when the master stops. `Hat`
  therefore checks `USER_CTRL` `I2C_MST_EN` on every sample and treats four
  byte-identical samples as a stall. See
  [How a stalled magnetometer is detected](accuracy.md#how-a-stalled-magnetometer-is-detected).
- ST1 DRDY is no use as a check, because reading HX clears it.
- The magnetometer's y and z axes point opposite the ICM20948's. `Hat`
  negates them, so every axis in a `Motion` is in the board frame: x and y
  along the board, z out of its top face.

## Board quirks

- The Pi's processor (SoC) heats the board. During a soak run, the BME280
  read 49 °C while the SoC read 65 °C. `Hat` reports the BME280's
  measurement. To set the optional offset, see
  [how to set the temperature offset](api.md#config).
- Indoors, the UV counts read 0. That is expected away from daylight.
- One chip at a time owns the bus, for one transaction. A chip's
  conversion wait never holds the bus. For details, see
  [how the samplers share the bus](api.md#sampling).

[hat]: https://www.waveshare.com/wiki/Environment_Sensor_HAT
