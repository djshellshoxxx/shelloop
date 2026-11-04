//! Euclidean rhythms for the musical `euclid HITS STEPS ...` form.
//!
//! Thin adapter over [`crate::euclid::euclidean_mask`] (spec 16, Bjorklund)
//! so the product has exactly one Euclidean algorithm and one rotation
//! convention.

pub use crate::euclid::MAX_EUCLID_STEPS;

pub fn euclidean(hits: usize, steps: usize, rotation: usize) -> Result<Vec<bool>, String> {
    crate::euclid::euclidean_mask(hits, steps, rotation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(pattern: &[bool]) -> String {
        pattern
            .iter()
            .map(|hit| if *hit { 'x' } else { '.' })
            .collect()
    }

    #[test]
    fn classic_patterns() {
        assert_eq!(text(&euclidean(3, 8, 0).unwrap()), "x..x..x.");
        assert_eq!(text(&euclidean(4, 4, 0).unwrap()), "xxxx");
        assert_eq!(text(&euclidean(0, 4, 0).unwrap()), "....");
    }

    #[test]
    fn rotation_matches_spec_16_and_wraps() {
        let base = euclidean(3, 8, 0).unwrap();
        let rotated = euclidean(3, 8, 1).unwrap();
        assert_eq!(rotated, crate::euclid::euclidean_mask(3, 8, 1).unwrap());
        assert_eq!(rotated[1], base[0]);
        assert_eq!(euclidean(3, 8, 8).unwrap(), base);
    }

    #[test]
    fn hit_count_always_matches() {
        for steps in 1..=32 {
            for hits in 0..=steps {
                let pattern = euclidean(hits, steps, 3).unwrap();
                assert_eq!(pattern.iter().filter(|hit| **hit).count(), hits);
            }
        }
    }

    #[test]
    fn bounds_are_enforced() {
        assert!(euclidean(9, 8, 0).is_err());
        assert!(euclidean(1, 0, 0).is_err());
        assert!(euclidean(1, MAX_EUCLID_STEPS + 1, 0).is_err());
    }
}
