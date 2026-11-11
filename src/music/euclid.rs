//! Euclidean rhythms: `hits` onsets spread as evenly as possible across
//! `steps`, then rotated. Uses the even-distribution formula
//! `hit(i) = (i * hits) mod steps < hits`, which gives the same maximally
//! even pattern as Bjorklund's algorithm up to rotation.

pub const MAX_EUCLID_STEPS: usize = 256;

pub fn euclidean(hits: usize, steps: usize, rotation: usize) -> Result<Vec<bool>, String> {
    if !(1..=MAX_EUCLID_STEPS).contains(&steps) {
        return Err(format!(
            "euclid steps must be between 1 and {MAX_EUCLID_STEPS}"
        ));
    }
    if hits > steps {
        return Err("euclid hits may not exceed steps".into());
    }
    let base: Vec<bool> = (0..steps)
        .map(|index| (index * hits) % steps < hits)
        .collect();
    let shift = rotation % steps;
    Ok((0..steps)
        .map(|index| base[(index + shift) % steps])
        .collect())
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
    fn five_of_eight_is_a_rotation_of_the_bjorklund_pattern() {
        // Bjorklund E(5,8) is x.xx.xx.; every rotation of it must match.
        let reference: Vec<char> = "x.xx.xx.".chars().collect();
        let ours = text(&euclidean(5, 8, 0).unwrap());
        let found = (0..8).any(|shift| {
            let rotated: String = (0..8).map(|i| reference[(i + shift) % 8]).collect();
            rotated == ours
        });
        assert!(found, "got {ours}");
    }

    #[test]
    fn rotation_shifts_left_and_wraps() {
        let base = euclidean(3, 8, 0).unwrap();
        let rotated = euclidean(3, 8, 1).unwrap();
        assert_eq!(rotated[0], base[1]);
        assert_eq!(rotated[7], base[0]);
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
