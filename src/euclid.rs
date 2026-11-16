//! Euclidean (Bjorklund) rhythm generation for pattern editing (spec 16).
//!
//! Pure control-thread code: no state, no allocation beyond the returned vector.

/// Maximum step count accepted by [`euclidean_mask`]. Matches the pattern limit.
pub const MAX_EUCLID_STEPS: usize = crate::pattern::MAX_PATTERN_STEPS;

/// Returns `steps` booleans with exactly `hits` set, distributed as evenly
/// as possible (Bjorklund), then rotated right by `rotation` steps.
///
/// `hits > steps`, `steps == 0` or `steps > MAX_EUCLID_STEPS` are rejected.
pub fn euclidean_mask(hits: usize, steps: usize, rotation: usize) -> Result<Vec<bool>, String> {
    if steps == 0 || steps > MAX_EUCLID_STEPS {
        return Err(format!(
            "euclid step count must be between 1 and {MAX_EUCLID_STEPS}"
        ));
    }
    if hits > steps {
        return Err(format!(
            "euclid hits ({hits}) cannot exceed the {steps} steps"
        ));
    }
    let mut mask = bjorklund(hits, steps);
    mask.rotate_right(rotation % steps);
    Ok(mask)
}

/// Bjorklund's algorithm on two sequences of "hit" and "rest" groups.
fn bjorklund(hits: usize, steps: usize) -> Vec<bool> {
    if hits == 0 {
        return vec![false; steps];
    }
    // Each group is a run of booleans. Start with `hits` single hits and
    // `steps - hits` single rests, then repeatedly merge pairs.
    let mut front: Vec<Vec<bool>> = vec![vec![true]; hits];
    let mut back: Vec<Vec<bool>> = vec![vec![false]; steps - hits];
    while front.len().min(back.len()) > 1 {
        let pairs = front.len().min(back.len());
        let mut merged: Vec<Vec<bool>> = Vec::with_capacity(pairs);
        for i in 0..pairs {
            let mut group = front[i].clone();
            group.extend_from_slice(&back[i]);
            merged.push(group);
        }
        // The unmatched tail of the longer side becomes the new "back" group.
        let remainder: Vec<Vec<bool>> = if front.len() > pairs {
            front.split_off(pairs)
        } else {
            back.split_off(pairs)
        };
        front = merged;
        back = remainder;
    }
    front.into_iter().chain(back).flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(mask: &[bool]) -> String {
        mask.iter()
            .map(|hit| if *hit { 'x' } else { '.' })
            .collect()
    }

    #[test]
    fn canonical_patterns_match_the_literature() {
        assert_eq!(text(&euclidean_mask(3, 8, 0).unwrap()), "x..x..x.");
        assert_eq!(text(&euclidean_mask(5, 8, 0).unwrap()), "x.xx.xx.");
        assert_eq!(text(&euclidean_mask(2, 5, 0).unwrap()), "x.x..");
    }

    #[test]
    fn hit_count_is_exact_for_every_size() {
        for steps in 1..=32 {
            for hits in 0..=steps {
                let mask = euclidean_mask(hits, steps, 0).unwrap();
                assert_eq!(mask.len(), steps);
                assert_eq!(
                    mask.iter().filter(|h| **h).count(),
                    hits,
                    "E({hits},{steps})"
                );
            }
        }
    }

    #[test]
    fn rotation_shifts_right_and_wraps() {
        let base = euclidean_mask(3, 8, 0).unwrap();
        assert_eq!(text(&euclidean_mask(3, 8, 1).unwrap()), ".x..x..x");
        assert_eq!(euclidean_mask(3, 8, 8).unwrap(), base);
        assert_eq!(
            euclidean_mask(3, 8, 9).unwrap(),
            euclidean_mask(3, 8, 1).unwrap()
        );
    }

    #[test]
    fn invalid_arguments_are_rejected() {
        assert!(euclidean_mask(9, 8, 0).is_err());
        assert!(euclidean_mask(1, 0, 0).is_err());
        assert!(euclidean_mask(1, MAX_EUCLID_STEPS + 1, 0).is_err());
    }
}
