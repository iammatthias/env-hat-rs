//! What the HAT measured: one optional group per chip. A group is `None`
//! while its chip is absent, failing, or hasn't produced a first sample.

/// A snapshot of the newest value from every chip.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reading {
    /// When the snapshot was taken, in milliseconds since the Unix epoch.
    pub unix_ms: u64,
    /// BME280.
    pub air: Option<Air>,
    /// TSL2591.
    pub light: Option<Light>,
    /// LTR390.
    pub uv: Option<Uv>,
    /// SGP40.
    pub gas: Option<Gas>,
    /// ICM20948 and its AK09916 magnetometer.
    pub motion: Option<Motion>,
}

/// Air at the BME280.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Air {
    /// Temperature in °C as the BME280 measured it, plus
    /// `Config::temperature_offset_c`. The Pi warms the board, so without
    /// an offset this reads above the room.
    pub temperature_c: f64,
    /// Relative humidity in % as the BME280 measured it. With a
    /// temperature offset, it is the humidity at `temperature_c` with the
    /// same dew point.
    pub humidity_pct: f64,
    /// Pressure at the board in hPa.
    pub pressure_hpa: f64,
    /// Dew point in °C. A temperature offset does not change it.
    pub dew_point_c: f64,
    /// Pressure reduced to sea level, in hPa. Set when `Config::altitude_m`
    /// is.
    pub sea_level_hpa: Option<f64>,
}

/// Light at the TSL2591.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    /// Lux from both channels, by Adafruit's formula (lux coefficient 408).
    /// Not calibrated against a lux meter.
    pub lux: f64,
    /// Full-spectrum channel counts at `gain`.
    pub full: u16,
    /// Infrared channel counts at `gain`.
    pub ir: u16,
    /// The analog gain auto-ranging settled on: 1, 25, 428, or 9,876
    /// (Adafruit's nominal values).
    pub gain: f64,
}

/// UV and ambient light at the LTR390.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Uv {
    /// UV index: UV counts at gain 18 over 1,400 counts per UVI
    /// (LTR-390UV-01 datasheet V1.2).
    pub index: f64,
    /// Raw UV counts, 20 bits.
    pub counts: u32,
    /// Lux from the ambient-light channel. `None` if its conversion timed
    /// out.
    pub ambient_lux: Option<f64>,
}

/// Volatile organic compounds at the SGP40.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gas {
    /// Sensirion VOC Index, 1 to 500, where 100 is this sensor's recent
    /// average air. `None` for the first 46 samples, while the algorithm
    /// returns 0.
    pub voc_index: Option<u16>,
    /// The SGP40's raw signal in ticks, compensated for the board's
    /// temperature and humidity.
    pub raw: u16,
}

/// Board frame: x and y along the board, z out of its top face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Motion {
    /// Rotation about x from gravity, in degrees. 0 when level.
    pub roll_deg: f64,
    /// Rotation about y from gravity, in degrees. 0 when level.
    pub pitch_deg: f64,
    /// Magnetic heading of the board's x axis, clockwise from magnetic
    /// north, 0 to 360. `None` without `Config::hard_iron_ut` or without
    /// `mag_ut`.
    pub heading_deg: Option<f64>,
    /// Specific force in g: about +1 on z when lying flat.
    pub accel_g: [f64; 3],
    /// Rotation rate in degrees per second, gyroscope offset removed.
    pub gyro_dps: [f64; 3],
    /// Magnetic field in µT. `None` while the field exceeds the AK09916's
    /// ±4,912 µT range, while the magnetometer is stalled, or before it has
    /// started.
    pub mag_ut: Option<[f64; 3]>,
    /// ICM20948 die temperature in °C, from the datasheet's untrimmed
    /// sensitivity.
    pub temperature_c: f64,
}
