//! Ratio conditions: a step plays only on loop cycles where
//! `loop % of == play - 1`. `1:2` plays on every other cycle (cycles 1, 3,
//! 5, ...), `3:4` on three cycles out of four.

use serde::{Deserialize, Serialize};

pub const MAX_RATIO_CYCLE: u8 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepRatio {
    pub step: u16,
    pub play: u8,
    pub of: u8,
}

impl StepRatio {
    pub fn validate(&self, len: usize) -> Result<(), String> {
        if usize::from(self.step) >= len {
            return Err(format!(
                "ratio references step {} beyond pattern length",
                self.step
            ));
        }
        validate_ratio(self.play, self.of)
    }

    pub fn fires(&self, loop_index: u64) -> bool {
        ratio_fires(self.play, self.of, loop_index)
    }
}

pub fn validate_ratio(play: u8, of: u8) -> Result<(), String> {
    if !(1..=MAX_RATIO_CYCLE).contains(&of) {
        return Err(format!(
            "ratio cycle must be between 1 and {MAX_RATIO_CYCLE}"
        ));
    }
    if !(1..=of).contains(&play) {
        return Err(format!("ratio play count must be between 1 and {of}"));
    }
    Ok(())
}

/// Whether a step with the ratio `play:of` sounds on `loop_index`.
pub fn ratio_fires(play: u8, of: u8, loop_index: u64) -> bool {
    if of == 0 {
        return false;
    }
    loop_index % u64::from(of) == u64::from(play) - 1
}

/// Parse `P:Q` (for example `1:2`).
pub fn parse_ratio(text: &str) -> Result<(u8, u8), String> {
    let (play, of) = text
        .split_once(':')
        .ok_or_else(|| format!("ratio must look like 1:2 or 3:4, got {text}"))?;
    let play: u8 = play.parse().map_err(|_| format!("invalid ratio: {text}"))?;
    let of: u8 = of.parse().map_err(|_| format!("invalid ratio: {text}"))?;
    validate_ratio(play, of)?;
    Ok((play, of))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_of_two_plays_on_odd_positions_in_one_based_terms() {
        let firing: Vec<u64> = (0..8)
            .filter(|loop_index| ratio_fires(1, 2, *loop_index))
            .collect();
        assert_eq!(firing, [0, 2, 4, 6]);
        let three_of_four: Vec<u64> = (0..8)
            .filter(|loop_index| ratio_fires(3, 4, *loop_index))
            .collect();
        assert_eq!(three_of_four, [2, 6]);
    }

    #[test]
    fn parsing_and_bounds() {
        assert_eq!(parse_ratio("1:2").unwrap(), (1, 2));
        assert_eq!(parse_ratio("3:4").unwrap(), (3, 4));
        assert!(parse_ratio("3:2").is_err());
        assert!(parse_ratio("0:2").is_err());
        assert!(parse_ratio("1:17").is_err());
        assert!(parse_ratio("12").is_err());
    }
}
