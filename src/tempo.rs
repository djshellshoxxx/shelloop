//! Phase-preserving musical time over a monotonically advancing sample clock.
//! Re-anchoring tempo never seeks or restarts the audio playhead.
use crate::QuantizeBoundary;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TempoClock {
    sample_rate: u32,
    bpm: f64,
    origin_frame: f64,
}

impl TempoClock {
    pub fn new(sample_rate: u32, bpm: f64) -> Result<Self, String> {
        crate::next_boundary_frame(0, sample_rate, bpm, 4, QuantizeBoundary::Immediate)?;
        Ok(Self {
            sample_rate,
            bpm,
            origin_frame: 0.0,
        })
    }
    pub(crate) fn from_snapshot(sample_rate: u32, bpm: f64, origin: f64) -> Option<Self> {
        if !origin.is_finite() {
            return None;
        }
        let mut clock = Self::new(sample_rate, bpm).ok()?;
        clock.origin_frame = origin;
        Some(clock)
    }
    pub fn sample_rate(self) -> u32 {
        self.sample_rate
    }
    pub fn bpm(self) -> f64 {
        self.bpm
    }
    pub fn origin_frame(self) -> f64 {
        self.origin_frame
    }
    pub fn frames_per_beat(self) -> f64 {
        f64::from(self.sample_rate) * 60.0 / self.bpm
    }
    pub fn beat_at_frame(self, frame: u64) -> f64 {
        (frame as f64 - self.origin_frame) / self.frames_per_beat()
    }
    pub fn frame_at_beat(self, beat: f64) -> u64 {
        (self.origin_frame + beat * self.frames_per_beat())
            .round()
            .clamp(0.0, u64::MAX as f64) as u64
    }
    pub(crate) fn next_bar_index(self, frame: u64, beats_per_bar: u32) -> u64 {
        let mut index = (self.beat_at_frame(frame) / f64::from(beats_per_bar))
            .floor()
            .max(0.0) as u64;
        for _ in 0..2 {
            if self.frame_at_beat(index as f64 * f64::from(beats_per_bar)) >= frame {
                break;
            }
            let Some(next) = index.checked_add(1) else {
                break;
            };
            index = next;
        }
        index
    }
    pub(crate) fn restart(&mut self) {
        self.origin_frame = 0.0;
    }
    pub fn set_bpm_at(&mut self, bpm: f64, frame: u64) -> Result<(), String> {
        let mut next = Self::new(self.sample_rate, bpm)?;
        next.origin_frame = frame as f64 - self.beat_at_frame(frame) * next.frames_per_beat();
        *self = next;
        Ok(())
    }
    pub fn next_boundary(
        self,
        frame: u64,
        steps_per_beat: u32,
        boundary: QuantizeBoundary,
    ) -> Result<u64, String> {
        crate::quantize::next_boundary_frame_from_origin(
            frame,
            self.sample_rate,
            self.bpm,
            steps_per_beat,
            boundary,
            self.origin_frame,
        )
    }
}
