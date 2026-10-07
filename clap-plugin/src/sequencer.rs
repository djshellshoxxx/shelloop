//! Host-synced step sequencer for the bundled pattern.
//!
//! The core [`shelloop::LiveSequencer`] runs on its own free-running frame
//! clock. A plugin must follow the host instead, so this sequencer works in
//! beats: the position is taken from the host transport (`song_pos_beats`,
//! `tempo`) at block start and on transport events, and advanced per sample
//! in between. Event timing mirrors [`shelloop::PatternScheduler`]: four steps
//! per beat, swing delays odd steps, ratchets subdivide a step, gate scales
//! each ratchet, and probability uses the same deterministic hash (with a
//! project seed of 0).
//!
//! Nothing here allocates after construction.

use shelloop::{EngineCommand, Pattern, PatternStep};

/// Steps per beat (sixteenth notes in 4/4).
pub const STEPS_PER_BEAT: f64 = 4.0;
const STEP_BEATS: f64 = 1.0 / STEPS_PER_BEAT;
/// Microtiming is clamped to half a step so the look-around window stays
/// bounded (the bundled pattern uses offsets of a few dozen frames).
const MAX_MICRO_BEATS: f64 = STEP_BEATS * 0.5;
/// Pending note-offs; a note-on that cannot reserve one is dropped, so a
/// note can never hang.
const PENDING_CAPACITY: usize = 64;
/// Position jumps larger than this (in beats) are treated as a relocation
/// (loop wrap, seek): sounding sequencer notes are released first.
const RELOCATE_THRESHOLD_BEATS: f64 = STEP_BEATS;
const MIN_TEMPO: f64 = 1.0;
const MAX_TEMPO: f64 = 999.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct PendingOff {
    beat: f64,
    note: u8,
}

/// Transport information relevant to the sequencer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransportInfo {
    pub playing: bool,
    pub song_pos_beats: f64,
    pub tempo: f64,
}

#[derive(Debug, Clone)]
pub struct HostSequencer {
    steps: Vec<Option<PatternStep>>,
    seed: u64,
    swing: f64,
    channel: u8,
    sample_rate: f64,
    enabled: bool,
    transport_playing: bool,
    beat: f64,
    beats_per_sample: f64,
    pending: [PendingOff; PENDING_CAPACITY],
    pending_len: usize,
}

impl HostSequencer {
    /// Main thread / `activate` only (allocates the step table).
    pub fn new(pattern: &Pattern, sample_rate: f64) -> Result<Self, String> {
        pattern.validate()?;
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("sample rate must be finite and positive".into());
        }
        Ok(Self {
            steps: pattern.steps.clone(),
            seed: pattern.seed,
            swing: f64::from(pattern.swing),
            channel: pattern.channel,
            sample_rate,
            enabled: false,
            transport_playing: false,
            beat: 0.0,
            beats_per_sample: 120.0 / 60.0 / sample_rate,
            pending: [PendingOff { beat: 0.0, note: 0 }; PENDING_CAPACITY],
            pending_len: 0,
        })
    }

    pub fn channel(&self) -> u8 {
        self.channel
    }

    pub fn is_running(&self) -> bool {
        self.enabled && self.transport_playing
    }

    pub fn sounding_notes(&self) -> usize {
        self.pending_len
    }

    pub fn beat(&self) -> f64 {
        self.beat
    }

    /// Turn note generation on or off. Turning it off releases every note
    /// the sequencer started.
    pub fn set_enabled(&mut self, enabled: bool, emit: &mut impl FnMut(EngineCommand)) {
        if self.enabled && !enabled {
            self.release_all(emit);
        }
        self.enabled = enabled;
    }

    /// Follow the host transport. `None` means the host sent no transport,
    /// which is treated as stopped.
    pub fn sync(&mut self, transport: Option<TransportInfo>, emit: &mut impl FnMut(EngineCommand)) {
        let Some(info) = transport.filter(|info| {
            info.playing && info.song_pos_beats.is_finite() && info.tempo.is_finite()
        }) else {
            if self.transport_playing {
                self.release_all(emit);
            }
            self.transport_playing = false;
            return;
        };

        self.beats_per_sample = info.tempo.clamp(MIN_TEMPO, MAX_TEMPO) / 60.0 / self.sample_rate;
        let jumped = (info.song_pos_beats - self.beat).abs() > RELOCATE_THRESHOLD_BEATS;
        if !self.transport_playing || jumped {
            self.release_all(emit);
        }
        self.beat = info.song_pos_beats;
        self.transport_playing = true;
    }

    /// Release every sounding sequencer note.
    pub fn release_all(&mut self, emit: &mut impl FnMut(EngineCommand)) {
        for pending in &self.pending[..self.pending_len] {
            emit(EngineCommand::NoteOff {
                channel: self.channel,
                note: pending.note,
            });
        }
        self.pending_len = 0;
    }

    /// Forget sounding notes without emitting note-offs (after a panic).
    pub fn clear(&mut self) {
        self.pending_len = 0;
    }

    /// Advance one sample, emitting the commands that fall inside it.
    pub fn tick(&mut self, emit: &mut impl FnMut(EngineCommand)) {
        if !self.transport_playing {
            return;
        }
        let start = self.beat;
        let end = start + self.beats_per_sample;

        // Note-offs first so a retriggered note does not cut itself.
        let mut index = self.pending_len;
        while index > 0 {
            index -= 1;
            if self.pending[index].beat < end {
                emit(EngineCommand::NoteOff {
                    channel: self.channel,
                    note: self.pending[index].note,
                });
                self.pending_len -= 1;
                self.pending[index] = self.pending[self.pending_len];
            }
        }

        if self.enabled && !self.steps.is_empty() {
            self.emit_note_ons(start, end, emit);
        }
        self.beat = end;
    }

    fn emit_note_ons(&mut self, start: f64, end: f64, emit: &mut impl FnMut(EngineCommand)) {
        let len = self.steps.len() as i64;
        let current = (start / STEP_BEATS).floor() as i64;
        let micro_scale = self.beats_per_sample;
        // An event of step g lies in [g - 0.5, g + 2.25) steps (microtiming,
        // swing up to 0.75 step, ratchets within the step), so steps
        // current-3 ..= current+1 cover every event that can start now.
        for global in (current - 3)..=(current + 1) {
            if global < 0 {
                continue;
            }
            let step_index = (global % len) as usize;
            let loop_index = (global / len) as u64;
            let Some(step) = self.steps[step_index] else {
                continue;
            };
            if !step_triggers(self.seed, loop_index, step_index, step.probability) {
                continue;
            }
            let swing = if step_index % 2 == 1 {
                self.swing * STEP_BEATS
            } else {
                0.0
            };
            let micro = (f64::from(step.microtiming_frames) * micro_scale)
                .clamp(-MAX_MICRO_BEATS, MAX_MICRO_BEATS);
            let step_start = global as f64 * STEP_BEATS + swing + micro;
            let ratchets = f64::from(step.ratchets.max(1));
            let spacing = STEP_BEATS / ratchets;
            // At least one sample long so the note-off lands on a later tick.
            let duration = (spacing * f64::from(step.gate)).max(self.beats_per_sample);
            for ratchet in 0..step.ratchets.max(1) {
                let at = step_start + spacing * f64::from(ratchet);
                if at < 0.0 || at < start || at >= end {
                    continue;
                }
                if self.pending_len >= PENDING_CAPACITY || step.velocity <= 0.0 {
                    continue;
                }
                emit(EngineCommand::NoteOn {
                    channel: self.channel,
                    note: step.note.min(127),
                    velocity: step.velocity.clamp(0.0, 1.0),
                });
                self.pending[self.pending_len] = PendingOff {
                    beat: at + duration,
                    note: step.note.min(127),
                };
                self.pending_len += 1;
            }
        }
    }
}

/// Same deterministic probability decision as the core scheduler.
fn step_triggers(pattern_seed: u64, loop_index: u64, step_index: usize, probability: f32) -> bool {
    if probability <= 0.0 {
        return false;
    }
    if probability >= 1.0 {
        return true;
    }
    let mut key = pattern_seed.rotate_left(17);
    key ^= loop_index.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    key ^= (step_index as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let random = splitmix64(key);
    let unit = (random >> 11) as f64 * (1.0 / ((1u64 << 53) as f64));
    unit < f64::from(probability)
}

fn splitmix64(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pattern() -> Pattern {
        shelloop::parse_pattern_json(crate::BUNDLED_PATTERN_JSON).expect("bundled pattern")
    }

    fn playing(beats: f64) -> Option<TransportInfo> {
        Some(TransportInfo {
            playing: true,
            song_pos_beats: beats,
            tempo: 120.0,
        })
    }

    fn run(seq: &mut HostSequencer, samples: usize, out: &mut Vec<(usize, EngineCommand)>) {
        for frame in 0..samples {
            seq.tick(&mut |command| out.push((frame, command)));
        }
    }

    #[test]
    fn first_step_fires_on_the_downbeat() {
        let mut seq = HostSequencer::new(&pattern(), 48_000.0).unwrap();
        let mut out = Vec::new();
        seq.set_enabled(true, &mut |c| out.push((0, c)));
        seq.sync(playing(0.0), &mut |c| out.push((0, c)));
        run(&mut seq, 10, &mut out);
        assert_eq!(
            out.first(),
            Some(&(
                0,
                EngineCommand::NoteOn {
                    channel: 1,
                    note: 36,
                    velocity: 0.9
                }
            ))
        );
    }

    #[test]
    fn plays_a_bar_with_balanced_note_offs() {
        let mut seq = HostSequencer::new(&pattern(), 48_000.0).unwrap();
        let mut out = Vec::new();
        seq.set_enabled(true, &mut |_| {});
        seq.sync(playing(0.0), &mut |_| {});
        // One 4/4 bar at 120 BPM = 2 s, plus a little tail.
        run(&mut seq, 48_000 * 2 + 4_800, &mut out);
        let ons = out
            .iter()
            .filter(|(_, c)| matches!(c, EngineCommand::NoteOn { .. }))
            .count();
        let offs = out
            .iter()
            .filter(|(_, c)| matches!(c, EngineCommand::NoteOff { .. }))
            .count();
        assert!(ons >= 4, "expected several notes, got {ons}");
        // The next bar's downbeat may have started; everything else is closed.
        assert!(ons - offs <= 1);
    }

    #[test]
    fn stopping_the_transport_releases_notes() {
        let mut seq = HostSequencer::new(&pattern(), 48_000.0).unwrap();
        seq.set_enabled(true, &mut |_| {});
        seq.sync(playing(0.0), &mut |_| {});
        run(&mut seq, 100, &mut Vec::new());
        assert_eq!(seq.sounding_notes(), 1);
        let mut released = Vec::new();
        seq.sync(None, &mut |c| released.push(c));
        assert_eq!(
            released,
            vec![EngineCommand::NoteOff {
                channel: 1,
                note: 36
            }]
        );
        assert_eq!(seq.sounding_notes(), 0);
        let mut after = Vec::new();
        run(&mut seq, 48_000, &mut after);
        assert!(after.is_empty());
    }

    #[test]
    fn relocation_releases_and_resumes_from_host_position() {
        let mut seq = HostSequencer::new(&pattern(), 48_000.0).unwrap();
        seq.set_enabled(true, &mut |_| {});
        seq.sync(playing(0.0), &mut |_| {});
        run(&mut seq, 100, &mut Vec::new());
        let mut released = Vec::new();
        // Jump to step 4 (beat 1.0): note 39.
        seq.sync(playing(1.0), &mut |c| released.push(c));
        assert_eq!(released.len(), 1);
        let mut out = Vec::new();
        run(&mut seq, 10, &mut out);
        assert!(matches!(
            out.first(),
            Some((0, EngineCommand::NoteOn { note: 39, .. }))
        ));
    }

    #[test]
    fn disabled_sequencer_is_silent_but_tracks_position() {
        let mut seq = HostSequencer::new(&pattern(), 48_000.0).unwrap();
        seq.sync(playing(0.0), &mut |_| {});
        let mut out = Vec::new();
        run(&mut seq, 24_000, &mut out);
        assert!(out.is_empty());
        assert!((seq.beat() - 1.0).abs() < 1e-6);
    }
}
