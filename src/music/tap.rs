//! Tap tempo: average the intervals between recent taps. Pure; the caller
//! supplies the time in milliseconds.

pub const TAP_WINDOW: usize = 8;
/// A pause longer than this starts a new measurement.
pub const TAP_TIMEOUT_MS: u64 = 2_000;
const MIN_BPM: f64 = 20.0;
const MAX_BPM: f64 = 400.0;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TapTempo {
    taps: [u64; TAP_WINDOW],
    count: usize,
}

impl TapTempo {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a tap. Returns the estimated BPM once two taps are in the
    /// current window, or `None` when there is not enough evidence or the
    /// tempo is outside 20..=400.
    pub fn tap(&mut self, now_ms: u64) -> Option<f64> {
        if self.count > 0 && now_ms.saturating_sub(self.taps[self.count - 1]) > TAP_TIMEOUT_MS {
            self.count = 0;
        }
        if self.count == TAP_WINDOW {
            self.taps.rotate_left(1);
            self.count -= 1;
        }
        self.taps[self.count] = now_ms;
        self.count += 1;
        if self.count < 2 {
            return None;
        }
        let first = self.taps[0];
        let last = self.taps[self.count - 1];
        let mean_interval_ms = (last - first) as f64 / (self.count - 1) as f64;
        if mean_interval_ms <= 0.0 {
            return None;
        }
        let bpm = 60_000.0 / mean_interval_ms;
        ((MIN_BPM..=MAX_BPM).contains(&bpm)).then(|| (bpm * 100.0).round() / 100.0)
    }

    pub fn reset(&mut self) {
        self.count = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_taps_give_the_tempo() {
        let mut tap = TapTempo::new();
        assert_eq!(tap.tap(1_000), None);
        assert_eq!(tap.tap(1_500), Some(120.0));
        assert_eq!(tap.tap(2_000), Some(120.0));
        assert_eq!(tap.tap(2_500), Some(120.0));
    }

    #[test]
    fn long_pause_restarts_the_measurement() {
        let mut tap = TapTempo::new();
        tap.tap(0);
        tap.tap(500);
        assert_eq!(tap.tap(10_000), None);
        assert_eq!(tap.tap(10_250), Some(240.0));
    }

    #[test]
    fn out_of_range_tempo_is_rejected() {
        let mut fast = TapTempo::new();
        fast.tap(0);
        assert_eq!(fast.tap(100), None); // 600 BPM
        let mut slow = TapTempo::new();
        slow.tap(0);
        assert_eq!(slow.tap(4_000), None); // 15 BPM
        let mut edge = TapTempo::new();
        edge.tap(0);
        assert_eq!(edge.tap(2_000), Some(30.0)); // 30 BPM is the lower bound
    }

    #[test]
    fn window_is_bounded() {
        let mut tap = TapTempo::new();
        for index in 0..100u64 {
            tap.tap(index * 500);
        }
        assert!(tap.count <= TAP_WINDOW);
    }
}
