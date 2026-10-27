//! The VOC baseline file under `Config::state_dir`: the save time in Unix
//! milliseconds, then the baseline's mean and std, separated by spaces.

use std::io;
use std::path::Path;

const VOC_FILE: &str = "voc.state";
/// Sensirion: don't restore a baseline after more than 10 minutes off.
const VOC_MAX_AGE_MS: u64 = 10 * 60 * 1000;

/// The VOC baseline `(mean, std)` saved under `dir`, if saved less than
/// 10 minutes before `now_ms`. A mean outside the SGP40's 16-bit signal
/// range, or a std that is not positive, would poison the estimator, so
/// such a file counts as no state.
pub(crate) fn load_voc(dir: &Path, now_ms: u64) -> Option<(f32, f32)> {
    let text = std::fs::read_to_string(dir.join(VOC_FILE)).ok()?;
    let [saved_ms, mean, std] = text.split_whitespace().collect::<Vec<_>>()[..] else {
        return None;
    };
    let age_ms = now_ms.checked_sub(saved_ms.parse().ok()?)?;
    let (mean, std): (f32, f32) = (mean.parse().ok()?, std.parse().ok()?);
    let plausible = (0.0..=65535.0).contains(&mean) && std > 0.0 && std <= 65535.0;
    (age_ms < VOC_MAX_AGE_MS && plausible).then_some((mean, std))
}

/// Saves the VOC baseline atomically: a reader sees the old file or the
/// new one, never a torn write. Creates `dir` if needed.
pub(crate) fn save_voc(dir: &Path, now_ms: u64, (mean, std): (f32, f32)) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    // f32's Display prints digits that parse back to the same f32.
    let tmp = dir.join(format!("{VOC_FILE}.tmp"));
    std::fs::write(&tmp, format!("{now_ms} {mean} {std}\n"))?;
    std::fs::rename(tmp, dir.join(VOC_FILE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::temp_dir;

    #[test]
    fn voc_state_round_trips_exactly_while_fresh() {
        let dir = temp_dir("voc");
        assert_eq!(load_voc(&dir, 0), None);
        let state = (11102.89_f32, 4151.1953_f32);
        save_voc(&dir, 1_000_000, state).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join(VOC_FILE)).unwrap(), "1000000 11102.89 4151.1953\n");
        assert_eq!(load_voc(&dir, 1_000_000), Some(state));
        assert_eq!(load_voc(&dir, 1_000_000 + 599_999), Some(state));
        assert_eq!(load_voc(&dir, 1_000_000 + 600_000), None);
        assert_eq!(load_voc(&dir, 999_999), None, "saved in the future");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn anything_but_three_numbers_is_no_state() {
        let dir = temp_dir("voc-bad");
        for bad in ["", "1000000 11102.89", "1000000 11102.89 4151.2 1", "1000000 x 4151.2", "-1 11102.89 4151.2"] {
            std::fs::write(dir.join(VOC_FILE), bad).unwrap();
            assert_eq!(load_voc(&dir, 1_000_000), None, "{bad:?}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_baseline_that_would_poison_the_estimator_is_no_state() {
        let dir = temp_dir("voc-hostile");
        // 1e300 is finite as text but infinite as an f32; a std of -220
        // cancels the algorithm's +220 bonus and divides by zero.
        for bad in ["1000000 1e300 4151.2", "1000000 11102.89 -220", "1000000 11102.89 0", "1000000 -1 4151.2"] {
            std::fs::write(dir.join(VOC_FILE), bad).unwrap();
            assert_eq!(load_voc(&dir, 1_000_000), None, "{bad:?}");
        }
        std::fs::write(dir.join(VOC_FILE), "1000000 65535 65535").unwrap();
        assert_eq!(load_voc(&dir, 1_000_000), Some((65535.0, 65535.0)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_creates_the_state_dir() {
        let dir = temp_dir("voc-mkdir").join("nested");
        save_voc(&dir, 1_000_000, (1.0, 2.0)).unwrap();
        assert_eq!(load_voc(&dir, 1_000_000), Some((1.0, 2.0)));
        std::fs::remove_dir_all(dir.parent().unwrap()).unwrap();
    }
}
