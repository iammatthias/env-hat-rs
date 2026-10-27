//! The names and units to record each value under, in any time-series
//! database. [`Reading::values`] pairs every present value with its
//! [`Field`]. Writing the values to a database is up to the caller.
//!
//! A name is ASCII snake_case: the group, the quantity, and the unit, as in
//! `air_temperature_c`. That is a valid Prometheus metric name, InfluxDB
//! field key, and SQL column. A name never changes once published, because
//! recorded history depends on it.

use crate::reading::Reading;

/// One recorded quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Field {
    /// The name to record the value under.
    pub name: &'static str,
    /// The value's unit, for display.
    pub unit: &'static str,
}

macro_rules! fields {
    ($($id:ident = $name:literal, $unit:literal;)*) => {
        $(
            #[doc = concat!("`", $name, "`, in ", $unit, ".")]
            pub const $id: Field = Field { name: $name, unit: $unit };
        )*
        /// Every field, in the order [`Reading::values`] yields them.
        pub const ALL: &[Field] = &[$($id),*];
    };
}

fields! {
    AIR_TEMPERATURE_C = "air_temperature_c", "°C";
    AIR_HUMIDITY_PCT = "air_humidity_pct", "%";
    AIR_PRESSURE_HPA = "air_pressure_hpa", "hPa";
    AIR_DEW_POINT_C = "air_dew_point_c", "°C";
    AIR_SEA_LEVEL_PRESSURE_HPA = "air_sea_level_pressure_hpa", "hPa";
    LIGHT_LUX = "light_lux", "lx";
    LIGHT_FULL_COUNTS = "light_full_counts", "counts";
    LIGHT_IR_COUNTS = "light_ir_counts", "counts";
    LIGHT_GAIN = "light_gain", "times";
    UV_INDEX = "uv_index", "UV index";
    UV_COUNTS = "uv_counts", "counts";
    UV_AMBIENT_LUX = "uv_ambient_lux", "lx";
    GAS_VOC_INDEX = "gas_voc_index", "VOC Index";
    GAS_RAW = "gas_raw", "counts";
    MOTION_ROLL_DEG = "motion_roll_deg", "°";
    MOTION_PITCH_DEG = "motion_pitch_deg", "°";
    MOTION_HEADING_DEG = "motion_heading_deg", "°";
    MOTION_ACCEL_X_G = "motion_accel_x_g", "g";
    MOTION_ACCEL_Y_G = "motion_accel_y_g", "g";
    MOTION_ACCEL_Z_G = "motion_accel_z_g", "g";
    MOTION_GYRO_X_DPS = "motion_gyro_x_dps", "°/s";
    MOTION_GYRO_Y_DPS = "motion_gyro_y_dps", "°/s";
    MOTION_GYRO_Z_DPS = "motion_gyro_z_dps", "°/s";
    MOTION_MAG_X_UT = "motion_mag_x_ut", "µT";
    MOTION_MAG_Y_UT = "motion_mag_y_ut", "µT";
    MOTION_MAG_Z_UT = "motion_mag_z_ut", "µT";
    MOTION_TEMPERATURE_C = "motion_temperature_c", "°C";
}

impl Reading {
    /// Every present value with its [`Field`], in [`ALL`] order. A group
    /// that is `None`, and a field that is `None` within its group, yield
    /// nothing. The timestamp is [`Reading::unix_ms`].
    ///
    /// ```
    /// use env_hat::{Air, Reading};
    ///
    /// let air = Air { temperature_c: 38.1, humidity_pct: 17.7, pressure_hpa: 1003.9, dew_point_c: 9.4, sea_level_hpa: None };
    /// let reading = Reading { unix_ms: 1_793_116_770_000, air: Some(air), ..Reading::default() };
    /// // InfluxDB line protocol, with the timestamp in milliseconds.
    /// let fields: Vec<String> = reading.values().iter().map(|(f, v)| format!("{}={v}", f.name)).collect();
    /// let line = format!("env_hat {} {}", fields.join(","), reading.unix_ms);
    /// assert_eq!(line, "env_hat air_temperature_c=38.1,air_humidity_pct=17.7,air_pressure_hpa=1003.9,air_dew_point_c=9.4 1793116770000");
    /// ```
    pub fn values(&self) -> Vec<(Field, f64)> {
        let mut v = Vec::with_capacity(ALL.len());
        if let Some(a) = &self.air {
            v.extend([
                (AIR_TEMPERATURE_C, a.temperature_c),
                (AIR_HUMIDITY_PCT, a.humidity_pct),
                (AIR_PRESSURE_HPA, a.pressure_hpa),
                (AIR_DEW_POINT_C, a.dew_point_c),
            ]);
            v.extend(a.sea_level_hpa.map(|p| (AIR_SEA_LEVEL_PRESSURE_HPA, p)));
        }
        if let Some(l) = &self.light {
            v.extend([(LIGHT_LUX, l.lux), (LIGHT_FULL_COUNTS, l.full.into()), (LIGHT_IR_COUNTS, l.ir.into()), (LIGHT_GAIN, l.gain)]);
        }
        if let Some(u) = &self.uv {
            v.extend([(UV_INDEX, u.index), (UV_COUNTS, u.counts.into())]);
            v.extend(u.ambient_lux.map(|l| (UV_AMBIENT_LUX, l)));
        }
        if let Some(g) = &self.gas {
            v.extend(g.voc_index.map(|i| (GAS_VOC_INDEX, i.into())));
            v.push((GAS_RAW, g.raw.into()));
        }
        if let Some(m) = &self.motion {
            v.extend([(MOTION_ROLL_DEG, m.roll_deg), (MOTION_PITCH_DEG, m.pitch_deg)]);
            v.extend(m.heading_deg.map(|h| (MOTION_HEADING_DEG, h)));
            v.extend([MOTION_ACCEL_X_G, MOTION_ACCEL_Y_G, MOTION_ACCEL_Z_G].into_iter().zip(m.accel_g));
            v.extend([MOTION_GYRO_X_DPS, MOTION_GYRO_Y_DPS, MOTION_GYRO_Z_DPS].into_iter().zip(m.gyro_dps));
            if let Some(mag) = m.mag_ut {
                v.extend([MOTION_MAG_X_UT, MOTION_MAG_Y_UT, MOTION_MAG_Z_UT].into_iter().zip(mag));
            }
            v.push((MOTION_TEMPERATURE_C, m.temperature_c));
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reading::{Air, Gas, Light, Motion, Uv};

    fn full() -> Reading {
        Reading {
            unix_ms: 1_700_000_000_000,
            air: Some(Air { temperature_c: 38.1, humidity_pct: 17.7, pressure_hpa: 1003.9, dew_point_c: 9.4, sea_level_hpa: Some(1015.0) }),
            light: Some(Light { lux: 120.5, full: 5000, ir: 1000, gain: 25.0 }),
            uv: Some(Uv { index: 0.5, counts: 700, ambient_lux: Some(940.0) }),
            gas: Some(Gas { voc_index: Some(100), raw: 31587 }),
            motion: Some(Motion {
                roll_deg: 1.0,
                pitch_deg: -2.0,
                heading_deg: Some(270.0),
                accel_g: [0.01, 0.02, 0.99],
                gyro_dps: [0.1, 0.2, 0.3],
                mag_ut: Some([20.0, -3.0, -40.0]),
                temperature_c: 41.5,
            }),
        }
    }

    /// Pins every name, its unit, and the value it carries. Databases hold
    /// history under these names, so this list may grow but never change.
    #[test]
    fn a_full_reading_yields_every_field_under_its_published_name() {
        let got: Vec<_> = full().values().into_iter().map(|(f, v)| (f.name, f.unit, v)).collect();
        assert_eq!(
            got,
            [
                ("air_temperature_c", "°C", 38.1),
                ("air_humidity_pct", "%", 17.7),
                ("air_pressure_hpa", "hPa", 1003.9),
                ("air_dew_point_c", "°C", 9.4),
                ("air_sea_level_pressure_hpa", "hPa", 1015.0),
                ("light_lux", "lx", 120.5),
                ("light_full_counts", "counts", 5000.0),
                ("light_ir_counts", "counts", 1000.0),
                ("light_gain", "times", 25.0),
                ("uv_index", "UV index", 0.5),
                ("uv_counts", "counts", 700.0),
                ("uv_ambient_lux", "lx", 940.0),
                ("gas_voc_index", "VOC Index", 100.0),
                ("gas_raw", "counts", 31587.0),
                ("motion_roll_deg", "°", 1.0),
                ("motion_pitch_deg", "°", -2.0),
                ("motion_heading_deg", "°", 270.0),
                ("motion_accel_x_g", "g", 0.01),
                ("motion_accel_y_g", "g", 0.02),
                ("motion_accel_z_g", "g", 0.99),
                ("motion_gyro_x_dps", "°/s", 0.1),
                ("motion_gyro_y_dps", "°/s", 0.2),
                ("motion_gyro_z_dps", "°/s", 0.3),
                ("motion_mag_x_ut", "µT", 20.0),
                ("motion_mag_y_ut", "µT", -3.0),
                ("motion_mag_z_ut", "µT", -40.0),
                ("motion_temperature_c", "°C", 41.5),
            ]
        );
        let fields: Vec<Field> = full().values().into_iter().map(|(f, _)| f).collect();
        assert_eq!(fields, ALL);
    }

    #[test]
    fn absent_groups_and_values_yield_nothing() {
        assert_eq!(Reading::default().values(), []);
        let mut r = full();
        r.air.as_mut().unwrap().sea_level_hpa = None;
        r.light = None;
        r.uv.as_mut().unwrap().ambient_lux = None;
        r.gas.as_mut().unwrap().voc_index = None;
        let m = r.motion.as_mut().unwrap();
        (m.heading_deg, m.mag_ut) = (None, None);
        let names: Vec<_> = r.values().into_iter().map(|(f, _)| f.name).collect();
        let dropped = [
            "air_sea_level_pressure_hpa",
            "light_lux",
            "light_full_counts",
            "light_ir_counts",
            "light_gain",
            "uv_ambient_lux",
            "gas_voc_index",
            "motion_heading_deg",
            "motion_mag_x_ut",
            "motion_mag_y_ut",
            "motion_mag_z_ut",
        ];
        let want: Vec<_> = ALL.iter().map(|f| f.name).filter(|n| !dropped.contains(n)).collect();
        assert_eq!(names, want);
    }

    /// The docs table is what users copy into their database. It lists
    /// every field, with its unit, and nothing else.
    #[test]
    fn the_recording_guide_lists_every_field() {
        let doc = include_str!("../../../docs/recording.md");
        let rows: Vec<(&str, &str)> = doc
            .lines()
            .filter_map(|l| l.strip_prefix("| `"))
            .filter_map(|l| {
                let (name, rest) = l.split_once("` | ")?;
                Some((name, rest.split(" | ").next()?))
            })
            .collect();
        let want: Vec<_> = ALL.iter().map(|f| (f.name, f.unit)).collect();
        assert_eq!(rows, want);
    }
}
