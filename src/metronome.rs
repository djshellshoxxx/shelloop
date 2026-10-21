//! Metronome clicks and count-in, rendered on the audio thread.
//!
//! Both are sample-accurate and allocation-free: a click is a short decaying
//! sine at a fixed frequency, and a count-in is a frame counter that starts
//! the transport on the frame after its last beat. The engine owns both and
//! calls them once per frame.

use std::f64::consts::TAU;

/// Click length.
pub const CLICK_MS: f64 = 30.0;
/// Decay time constant of the click envelope.
const DECAY_MS: f64 = 8.0;
pub const BEAT_HZ: f64 = 1_000.0;
pub const DOWNBEAT_HZ: f64 = 1_500.0;
pub const MAX_COUNT_IN_BARS: u32 = 4;

#[derive(Debug, Clone)]
pub struct Metronome {
    sample_rate: f64,
    enabled: bool,
    gain: f32,
    beats_per_bar: u64,
    last_beat: Option<u64>,
    click_frames: u32,
    /// Samples since the current click started; `>= click_frames` is silent.
    age: u32,
    phase: f64,
    frequency: f64,
}

impl Metronome {
    pub fn new(sample_rate: u32, beats_per_bar: u32) -> Self {
        let sample_rate = f64::from(sample_rate.max(1));
        let click_frames = (CLICK_MS * sample_rate / 1_000.0).round() as u32;
        Self {
            sample_rate,
            enabled: false,
            gain: 0.5,
            beats_per_bar: u64::from(beats_per_bar.max(1)),
            last_beat: None,
            click_frames,
            age: click_frames,
            phase: 0.0,
            frequency: BEAT_HZ,
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn gain(&self) -> f32 {
        self.gain
    }

    pub fn set(&mut self, enabled: bool, gain: f32) -> Result<(), &'static str> {
        if !gain.is_finite() || !(0.0..=1.0).contains(&gain) {
            return Err("metronome gain must be finite and between 0.0 and 1.0");
        }
        self.enabled = enabled;
        self.gain = gain;
        Ok(())
    }

    /// Render one sample. `beat` is the beat index sounding at this frame,
    /// or `None` when no click should be running. A new beat index starts a
    /// click; the first beat of each bar is accented.
    pub fn next_sample(&mut self, beat: Option<u64>) -> f32 {
        match beat {
            Some(index) if self.last_beat != Some(index) => {
                self.last_beat = Some(index);
                self.age = 0;
                self.phase = 0.0;
                self.frequency = if index % self.beats_per_bar == 0 {
                    DOWNBEAT_HZ
                } else {
                    BEAT_HZ
                };
            }
            Some(_) => {}
            None => self.last_beat = None,
        }
        if self.age >= self.click_frames {
            return 0.0;
        }
        let seconds = f64::from(self.age) / self.sample_rate;
        let envelope = (-seconds / (DECAY_MS / 1_000.0)).exp();
        let value = self.phase.sin() * envelope * f64::from(self.gain);
        self.phase += TAU * self.frequency / self.sample_rate;
        self.age += 1;
        value as f32
    }
}

/// A count-in of whole bars before the transport starts.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CountIn {
    elapsed: u64,
    total: u64,
    frames_per_beat: f64,
}

impl CountIn {
    pub fn new(bars: u32, beats_per_bar: u32, frames_per_beat: f64) -> Result<Self, &'static str> {
        if bars == 0 || bars > MAX_COUNT_IN_BARS {
            return Err("count-in bars must be between 1 and 4");
        }
        if !frames_per_beat.is_finite() || frames_per_beat < 1.0 {
            return Err("count-in needs a positive tempo");
        }
        let beats = u64::from(bars) * u64::from(beats_per_bar.max(1));
        Ok(Self {
            elapsed: 0,
            total: (beats as f64 * frames_per_beat).round() as u64,
            frames_per_beat,
        })
    }

    /// Advance one frame. Returns the beat index sounding on this frame and
    /// whether the count-in ends after it.
    pub fn tick(&mut self) -> (u64, bool) {
        let beat = (self.elapsed as f64 / self.frames_per_beat).floor() as u64;
        self.elapsed += 1;
        (beat, self.elapsed >= self.total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn click_fires_once_per_beat_and_then_decays_to_silence() {
        let mut metronome = Metronome::new(48_000, 4);
        metronome.set(true, 1.0).unwrap();
        let mut peak = 0.0_f32;
        let mut first = 0.0_f32;
        for frame in 0..48_000 {
            let sample = metronome.next_sample(Some(0));
            if frame == 0 {
                first = sample;
            }
            peak = peak.max(sample.abs());
        }
        assert!(peak > 0.1, "click should be audible");
        assert_eq!(first, 0.0, "sine starts at phase zero");
        // Silent after the click length while the beat stays the same.
        assert_eq!(metronome.next_sample(Some(0)), 0.0);
    }

    #[test]
    fn new_beat_retriggers_and_bars_are_accented() {
        let mut metronome = Metronome::new(48_000, 4);
        metronome.set(true, 1.0).unwrap();
        metronome.next_sample(Some(0));
        for _ in 0..2_000 {
            metronome.next_sample(Some(0));
        }
        // A new beat index restarts the click at phase zero.
        assert_eq!(metronome.next_sample(Some(1)), 0.0);
        assert_eq!(metronome.age, 1);
        assert_eq!(metronome.frequency, BEAT_HZ);
        metronome.next_sample(Some(4));
        assert_eq!(metronome.frequency, DOWNBEAT_HZ);
    }

    #[test]
    fn gain_is_validated() {
        let mut metronome = Metronome::new(48_000, 4);
        assert!(metronome.set(true, 1.5).is_err());
        assert!(metronome.set(true, f32::NAN).is_err());
        assert!(metronome.set(false, 0.0).is_ok());
    }

    #[test]
    fn count_in_length_and_end_flag() {
        let mut count_in = CountIn::new(1, 4, 100.0).unwrap();
        let mut ticks = 0_u64;
        let mut ended_at = None;
        loop {
            let (beat, ended) = count_in.tick();
            ticks += 1;
            if ended {
                ended_at = Some(ticks);
                assert_eq!(beat, 3, "last frame is in the last beat");
                break;
            }
        }
        assert_eq!(ended_at, Some(400));
        assert!(CountIn::new(0, 4, 100.0).is_err());
        assert!(CountIn::new(5, 4, 100.0).is_err());
    }
}
