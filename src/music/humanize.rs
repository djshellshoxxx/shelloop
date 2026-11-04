//! Humanize: deterministic timing and velocity jitter per step.
//!
//! Offsets are keyed on the pattern seed, project seed, loop index and step
//! index, never on the block being rendered. The same step therefore gets
//! the same jitter however the audio is partitioned into blocks.

use serde::{Deserialize, Serialize};

/// Maximum timing jitter (either side), 200 ms at 48 kHz.
pub const MAX_HUMANIZE_TIMING_FRAMES: u32 = 9_600;
/// Maximum velocity jitter (either side).
pub const MAX_HUMANIZE_VELOCITY: f32 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Humanize {
    /// Maximum timing jitter in frames, applied as +/- this amount.
    #[serde(default)]
    pub timing_frames: u32,
    /// Maximum velocity jitter, applied as +/- this amount.
    #[serde(default)]
    pub velocity: f32,
}

impl Humanize {
    pub fn is_default(&self) -> bool {
        self.timing_frames == 0 && self.velocity == 0.0
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.timing_frames > MAX_HUMANIZE_TIMING_FRAMES {
            return Err(format!(
                "humanize timing may not exceed {MAX_HUMANIZE_TIMING_FRAMES} frames"
            ));
        }
        if !self.velocity.is_finite() || !(0.0..=MAX_HUMANIZE_VELOCITY).contains(&self.velocity) {
            return Err(format!(
                "humanize velocity must be finite and 0.0..={MAX_HUMANIZE_VELOCITY}"
            ));
        }
        Ok(())
    }

    /// Timing offset in frames, within +/- `timing_frames`, for a step key.
    pub fn timing_offset(&self, key: u64) -> i64 {
        if self.timing_frames == 0 {
            return 0;
        }
        (keyed_unit(key) * f64::from(self.timing_frames)).round() as i64
    }

    /// Velocity change, within +/- `velocity`, for a step key.
    pub fn velocity_delta(&self, key: u64) -> f32 {
        if self.velocity == 0.0 {
            return 0.0;
        }
        (keyed_unit(key) as f32) * self.velocity
    }
}

/// A value in `[-1, 1)` derived from `key`.
pub fn keyed_unit(key: u64) -> f64 {
    let bits = splitmix64(key) >> 11;
    bits as f64 * (1.0 / (1u64 << 53) as f64) * 2.0 - 1.0
}

/// SplitMix64 finaliser; a cheap, well-mixed hash for keyed randomness.
pub(crate) fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = value;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_inert() {
        let humanize = Humanize::default();
        assert!(humanize.is_default());
        assert_eq!(humanize.timing_offset(12345), 0);
        assert_eq!(humanize.velocity_delta(12345), 0.0);
    }

    #[test]
    fn offsets_are_bounded_and_deterministic() {
        let humanize = Humanize {
            timing_frames: 120,
            velocity: 0.2,
        };
        for key in 0..10_000u64 {
            let offset = humanize.timing_offset(key);
            assert!(offset.abs() <= 120);
            assert_eq!(offset, humanize.timing_offset(key));
            let delta = humanize.velocity_delta(key);
            assert!(delta.abs() <= 0.2);
        }
        let spread: std::collections::HashSet<i64> = (0..1_000u64)
            .map(|key| humanize.timing_offset(key))
            .collect();
        assert!(spread.len() > 50, "jitter should spread across the range");
    }

    #[test]
    fn validation_rejects_out_of_range() {
        assert!(Humanize {
            timing_frames: MAX_HUMANIZE_TIMING_FRAMES + 1,
            velocity: 0.0
        }
        .validate()
        .is_err());
        assert!(Humanize {
            timing_frames: 0,
            velocity: 0.6
        }
        .validate()
        .is_err());
        assert!(Humanize {
            timing_frames: 0,
            velocity: f32::NAN
        }
        .validate()
        .is_err());
        assert!(Humanize {
            timing_frames: 480,
            velocity: 0.3
        }
        .validate()
        .is_ok());
    }
}
