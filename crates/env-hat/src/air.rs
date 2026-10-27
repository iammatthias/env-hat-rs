//! Derived air quantities: the dew point, a temperature offset that keeps
//! it, and sea-level pressure.

// Magnus formula coefficients over water (Sonntag 1990, as adopted by WMO).
const MAGNUS_A: f64 = 17.62;
const MAGNUS_B: f64 = 243.12;

fn magnus(temp_c: f64) -> f64 {
    MAGNUS_A * temp_c / (MAGNUS_B + temp_c)
}

/// Dew point (°C) of air at `temp_c` and relative humidity `rh_pct` > 0.
pub(crate) fn dew_point_c(temp_c: f64, rh_pct: f64) -> f64 {
    let g = (rh_pct / 100.0).ln() + magnus(temp_c);
    MAGNUS_B * g / (MAGNUS_A - g)
}

/// Relative humidity (%) of air at `temp_c` with dew point `dew_point_c`,
/// capped at 100 %.
pub(crate) fn relative_humidity_pct(temp_c: f64, dew_point_c: f64) -> f64 {
    (100.0 * (magnus(dew_point_c) - magnus(temp_c)).exp()).min(100.0)
}

/// Air at `temp_c` and `rh_pct`, warmed or cooled by `offset_c` at a
/// constant dew point. A zero offset returns the inputs unchanged.
pub(crate) fn offset(temp_c: f64, rh_pct: f64, offset_c: f64) -> (f64, f64) {
    if offset_c == 0.0 {
        return (temp_c, rh_pct);
    }
    let t = temp_c + offset_c;
    (t, relative_humidity_pct(t, dew_point_c(temp_c, rh_pct)))
}

/// Station pressure reduced to sea level by the hypsometric formula with a
/// standard lapse rate, using the station's air temperature.
pub(crate) fn sea_level_hpa(pressure_hpa: f64, altitude_m: f64, temp_c: f64) -> f64 {
    let lapse = 0.0065 * altitude_m;
    pressure_hpa * (1.0 - lapse / (temp_c + lapse + 273.15)).powf(-5.257)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::round;

    #[test]
    fn dew_point_matches_the_psychrometric_table() {
        assert_eq!(round(dew_point_c(25.0, 50.0), 1), 13.9);
    }

    #[test]
    fn humidity_at_a_warmer_or_cooler_temperature_keeps_the_dew_point() {
        assert_eq!(round(relative_humidity_pct(20.0, 13.85), 1), 67.7);
        assert_eq!(relative_humidity_pct(10.0, 13.85), 100.0);
    }

    #[test]
    fn an_offset_moves_the_temperature_at_a_constant_dew_point() {
        // 25 °C at 50 % has a 13.85 °C dew point, which is 67.7 % at 20 °C.
        let (t, rh) = offset(25.0, 50.0, -5.0);
        assert_eq!((t, round(rh, 1)), (20.0, 67.7));
        assert_eq!(offset(25.0, 50.0, 0.0), (25.0, 50.0));
    }

    #[test]
    fn sea_level_pressure_at_100_m() {
        assert_eq!(round(sea_level_hpa(1000.0, 100.0, 15.0), 2), 1011.92);
    }
}
