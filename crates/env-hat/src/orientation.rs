//! Tilt from gravity and a tilt-compensated magnetic heading, in the board
//! frame: x and y along the board, z out of its top face, and the
//! accelerometer reading about +1 g on z when the board lies flat.

/// `(roll, pitch)` in degrees from the accelerometer's gravity vector.
pub(crate) fn roll_pitch_deg([ax, ay, az]: [f64; 3]) -> (f64, f64) {
    (ay.atan2(az).to_degrees(), (-ax).atan2(ay.hypot(az)).to_degrees())
}

fn cross([a0, a1, a2]: [f64; 3], [b0, b1, b2]: [f64; 3]) -> [f64; 3] {
    [a1 * b2 - a2 * b1, a2 * b0 - a0 * b2, a0 * b1 - a1 * b0]
}

/// Heading of the board's x axis in degrees clockwise from magnetic north,
/// 0..360, at any tilt. Up is the gravity reaction `accel`; east is
/// perpendicular to up and the field; north completes the frame. `None`
/// when x points straight up or down, or the field is vertical.
pub(crate) fn heading_deg(accel: [f64; 3], mag_ut: [f64; 3], hard_iron_ut: [f64; 3]) -> Option<f64> {
    let field = [0, 1, 2].map(|k| mag_ut[k] - hard_iron_ut[k]);
    let east = cross(field, accel);
    let north = cross(accel, east);
    // |north| = |accel| |east|, so scaling east's x by |accel| puts both
    // components on the same footing without normalizing either.
    let accel_len = accel.iter().map(|v| v * v).sum::<f64>().sqrt();
    let (e, n) = (east[0] * accel_len, north[0]);
    if e == 0.0 && n == 0.0 {
        return None;
    }
    Some(e.atan2(n).to_degrees().rem_euclid(360.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::round;

    #[test]
    fn roll_and_pitch_from_gravity() {
        assert_eq!(roll_pitch_deg([0.0, 0.0, 1.0]), (0.0, 0.0));
        assert_eq!(roll_pitch_deg([0.0, 1.0, 0.0]), (90.0, 0.0));
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let (roll, pitch) = roll_pitch_deg([-h, 0.0, h]);
        assert_eq!((roll, round(pitch, 9)), (0.0, 45.0));
        let (roll, pitch) = roll_pitch_deg([0.0, -h, h]);
        assert_eq!((round(roll, 9), pitch), (-45.0, 0.0));
    }

    /// A northern-hemisphere field: 20 µT horizontal, 40 µT down.
    const NORTH: [f64; 3] = [20.0, 0.0, -40.0];
    const UP: [f64; 3] = [0.0, 0.0, 1.0];

    #[test]
    fn level_heading_follows_the_x_axis() {
        assert_eq!(heading_deg(UP, NORTH, [0.0; 3]), Some(0.0));
        // x east: the horizontal field lies along +y.
        assert_eq!(heading_deg(UP, [0.0, 20.0, -40.0], [0.0; 3]), Some(90.0));
        assert_eq!(heading_deg(UP, [-20.0, 0.0, -40.0], [0.0; 3]), Some(180.0));
        assert_eq!(heading_deg(UP, [0.0, -20.0, -40.0], [0.0; 3]), Some(270.0));
    }

    #[test]
    fn heading_is_unchanged_by_roll_about_x() {
        // The x-east pair rolled 30° about x: (0, 0, 1) -> (0, 1/2, √3/2) and
        // (0, 20, -40) -> (0, 20√3/2 - 20, -10 - 40√3/2).
        let c = 3f64.sqrt() / 2.0;
        let h = heading_deg([0.0, 0.5, c], [0.0, 20.0 * c - 20.0, -10.0 - 40.0 * c], [0.0; 3]);
        assert_eq!(h.map(|x| round(x, 9)), Some(90.0));
    }

    #[test]
    fn heading_is_unchanged_by_pitch_about_y() {
        // The x-north pair pitched 45° nose down (pitch +45°): up -> (-√½, 0, √½), and the
        // field -> (20√½ + 40√½, 0, 20√½ - 40√½).
        let h = std::f64::consts::FRAC_1_SQRT_2;
        let heading = heading_deg([-h, 0.0, h], [60.0 * h, 0.0, -20.0 * h], [0.0; 3]);
        assert_eq!(heading.map(|x| round(x, 9)), Some(0.0));
    }

    #[test]
    fn hard_iron_offsets_are_removed_first() {
        assert_eq!(heading_deg(UP, [5.0, 17.0, -38.0], [5.0, -3.0, 2.0]), Some(90.0));
    }

    #[test]
    fn a_vertical_field_has_no_heading() {
        assert_eq!(heading_deg(UP, [0.0, 0.0, -40.0], [0.0; 3]), None);
    }
}
