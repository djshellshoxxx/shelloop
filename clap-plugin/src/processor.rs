//! Audio-thread state: the polyphonic synth, the host-synced sequencer and
//! the parameter values the audio thread currently uses.
//!
//! Everything here is safe Rust. The FFI layer decodes raw CLAP events into
//! [`HostEvent`]s and splits each block at event times; this module never
//! allocates, locks or panics after [`Processor::new`].

use crate::params::{build_patch, ParamId, ParamStore, ParamValues, POLYPHONY};
use crate::sequencer::{HostSequencer, TransportInfo};
use shelloop::{protect_master, CompiledSynthPatch, EngineCommand, Pattern, RealtimeSynth};

/// A host event decoded from the CLAP event list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HostEvent {
    /// CLAP note-on. `channel`/`key` of -1 are wildcards (ignored for note-on).
    NoteOn {
        channel: i16,
        key: i16,
        velocity: f64,
    },
    NoteOff {
        channel: i16,
        key: i16,
    },
    Choke {
        channel: i16,
        key: i16,
    },
    /// Raw MIDI 1.0 bytes.
    Midi([u8; 3]),
    ParamValue {
        id: u32,
        value: f64,
    },
    Transport(Option<TransportInfo>),
}

/// Master-volume smoothing time constant.
const MASTER_SMOOTHING_SECS: f32 = 0.005;

#[derive(Debug)]
pub struct Processor {
    sample_rate: f32,
    synth: RealtimeSynth,
    sequencer: HostSequencer,
    values: ParamValues,
    patch_dirty: bool,
    /// Host-held keys per MIDI channel (bit = key).
    held: [u128; 16],
    master: f32,
    master_coeff: f32,
}

impl Processor {
    /// Build at the host sample rate. Allocates; call from `activate`.
    pub fn new(sample_rate: f64, values: ParamValues, pattern: &Pattern) -> Result<Self, String> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("sample rate must be finite and positive".into());
        }
        let rate = sample_rate as f32;
        let patch = build_patch(&values, rate);
        let synth = RealtimeSynth::new_with_patch(rate, patch, POLYPHONY)?;
        let mut sequencer = HostSequencer::new(pattern, sample_rate)?;
        sequencer.set_enabled(values[ParamId::Sequencer.index()] >= 0.5, &mut |_| {});
        Ok(Self {
            sample_rate: rate,
            synth,
            sequencer,
            values,
            patch_dirty: false,
            held: [0; 16],
            master: values[ParamId::MasterVolume.index()] as f32,
            master_coeff: 1.0 - (-1.0 / (MASTER_SMOOTHING_SECS * rate)).exp(),
        })
    }

    pub fn sample_rate(&self) -> f32 {
        self.sample_rate
    }

    pub fn active_voices(&self) -> usize {
        self.synth.active_voice_count()
    }

    pub fn sequencer(&self) -> &HostSequencer {
        &self.sequencer
    }

    /// Adopt a full set of values (after a state load on the main thread).
    pub fn sync_params(&mut self, values: ParamValues) {
        self.values = values;
        self.patch_dirty = true;
        let synth = &mut self.synth;
        self.sequencer
            .set_enabled(values[ParamId::Sequencer.index()] >= 0.5, &mut |command| {
                synth.handle(command)
            });
    }

    pub fn handle(&mut self, event: HostEvent, store: &ParamStore) {
        match event {
            HostEvent::NoteOn {
                channel,
                key,
                velocity,
            } => {
                if let (Some(channel), Some(key)) = (exact_channel(channel), exact_key(key)) {
                    if velocity.is_finite() && velocity > 0.0 {
                        self.note_on(channel, key, velocity.min(1.0) as f32);
                    }
                }
            }
            HostEvent::NoteOff { channel, key } => self.release_matching(channel, key),
            HostEvent::Choke { channel, key } => {
                if channel < 0 && key < 0 {
                    self.panic();
                } else {
                    // The engine has no per-voice hard stop; release instead.
                    self.release_matching(channel, key);
                }
            }
            HostEvent::Midi(data) => self.handle_midi(data),
            HostEvent::ParamValue { id, value } => self.set_param(id, value, store),
            HostEvent::Transport(info) => {
                let synth = &mut self.synth;
                self.sequencer
                    .sync(info, &mut |command| synth.handle(command));
            }
        }
    }

    fn note_on(&mut self, channel: u8, key: u8, velocity: f32) {
        let bit = 1u128 << key;
        let held = &mut self.held[usize::from(channel)];
        if *held & bit != 0 {
            // Retrigger: release the previous voice so none is orphaned.
            self.synth
                .handle(EngineCommand::NoteOff { channel, note: key });
        }
        *held |= bit;
        self.synth.handle(EngineCommand::NoteOn {
            channel,
            note: key,
            velocity,
        });
    }

    fn release_matching(&mut self, channel: i16, key: i16) {
        for ch in 0..16u8 {
            if channel >= 0 && i16::from(ch) != channel {
                continue;
            }
            let mut held = self.held[usize::from(ch)];
            if key >= 0 {
                held &= exact_key(key).map_or(0, |key| 1u128 << key);
            }
            while held != 0 {
                let note = held.trailing_zeros() as u8;
                held &= !(1u128 << note);
                self.held[usize::from(ch)] &= !(1u128 << note);
                self.synth
                    .handle(EngineCommand::NoteOff { channel: ch, note });
            }
        }
    }

    fn handle_midi(&mut self, data: [u8; 3]) {
        let channel = data[0] & 0x0F;
        let a = data[1] & 0x7F;
        let b = data[2] & 0x7F;
        match data[0] & 0xF0 {
            0x90 if b > 0 => self.note_on(channel, a, f32::from(b) / 127.0),
            0x80 | 0x90 => self.release_matching(i16::from(channel), i16::from(a)),
            0xB0 => match a {
                64 => self.synth.handle(EngineCommand::Sustain {
                    channel,
                    enabled: b >= 64,
                }),
                120 | 123 => self.panic(),
                _ => {}
            },
            _ => {}
        }
    }

    fn set_param(&mut self, raw_id: u32, value: f64, store: &ParamStore) {
        let Some(id) = ParamId::from_raw(raw_id) else {
            return;
        };
        let Some(value) = id.spec().sanitize(value) else {
            return;
        };
        store.set(id, value);
        self.values[id.index()] = value;
        match id {
            ParamId::Sequencer => {
                let synth = &mut self.synth;
                self.sequencer
                    .set_enabled(value >= 0.5, &mut |command| synth.handle(command));
            }
            ParamId::MasterVolume => {}
            _ => self.patch_dirty = true,
        }
    }

    /// Stop every voice and forget held and sequenced notes.
    pub fn panic(&mut self) {
        self.synth.handle(EngineCommand::Panic);
        self.sequencer.clear();
        self.held = [0; 16];
    }

    /// Release sequencer notes (processing stopped).
    pub fn release_sequencer(&mut self) {
        let synth = &mut self.synth;
        self.sequencer
            .release_all(&mut |command| synth.handle(command));
    }

    fn apply_pending_patch(&mut self) {
        if !self.patch_dirty {
            return;
        }
        self.patch_dirty = false;
        let patch = build_patch(&self.values, self.sample_rate);
        // build_patch clamps every field, so this cannot fail in practice;
        // if it ever did, the change is ignored and the old patch stays.
        if let Ok(compiled) = CompiledSynthPatch::new(self.sample_rate, patch) {
            self.synth.apply_patch(compiled);
        }
    }

    /// Render `frames` samples, passing each finished mono sample (already
    /// through `protect_master`) to `write` with its index.
    pub fn render(&mut self, frames: std::ops::Range<usize>, mut write: impl FnMut(usize, f32)) {
        self.apply_pending_patch();
        let target = self.values[ParamId::MasterVolume.index()] as f32;
        for frame in frames {
            let synth = &mut self.synth;
            self.sequencer.tick(&mut |command| synth.handle(command));
            let sample = self.synth.next_sample();
            self.master += (target - self.master) * self.master_coeff;
            write(frame, protect_master(sample * self.master));
        }
    }
}

fn exact_channel(channel: i16) -> Option<u8> {
    u8::try_from(channel).ok().filter(|channel| *channel < 16)
}

fn exact_key(key: i16) -> Option<u8> {
    u8::try_from(key).ok().filter(|key| *key < 128)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::params::default_values;

    fn processor() -> Processor {
        let pattern = shelloop::parse_pattern_json(crate::BUNDLED_PATTERN_JSON).unwrap();
        Processor::new(48_000.0, default_values(), &pattern).unwrap()
    }

    fn render(processor: &mut Processor, frames: usize) -> Vec<f32> {
        let mut out = vec![0.0; frames];
        processor.render(0..frames, |index, sample| out[index] = sample);
        out
    }

    #[test]
    fn wildcard_note_off_releases_everything() {
        let store = ParamStore::default();
        let mut p = processor();
        for key in [60, 64, 67] {
            p.handle(
                HostEvent::NoteOn {
                    channel: 0,
                    key,
                    velocity: 0.8,
                },
                &store,
            );
        }
        assert_eq!(p.active_voices(), 3);
        p.handle(
            HostEvent::NoteOff {
                channel: -1,
                key: -1,
            },
            &store,
        );
        render(&mut p, 48_000);
        assert_eq!(p.active_voices(), 0);
    }

    #[test]
    fn repeated_note_on_does_not_orphan_voices() {
        let store = ParamStore::default();
        let mut p = processor();
        for _ in 0..3 {
            p.handle(HostEvent::Midi([0x90, 60, 100]), &store);
        }
        p.handle(HostEvent::Midi([0x80, 60, 0]), &store);
        render(&mut p, 48_000);
        assert_eq!(p.active_voices(), 0);
    }

    #[test]
    fn sustain_pedal_holds_until_released() {
        let store = ParamStore::default();
        let mut p = processor();
        p.handle(HostEvent::Midi([0xB0, 64, 127]), &store);
        p.handle(HostEvent::Midi([0x90, 60, 100]), &store);
        p.handle(HostEvent::Midi([0x90, 60, 0]), &store);
        render(&mut p, 24_000);
        assert_eq!(p.active_voices(), 1);
        p.handle(HostEvent::Midi([0xB0, 64, 0]), &store);
        render(&mut p, 48_000);
        assert_eq!(p.active_voices(), 0);
    }

    #[test]
    fn cc123_panics() {
        let store = ParamStore::default();
        let mut p = processor();
        p.handle(HostEvent::Midi([0x90, 60, 100]), &store);
        p.handle(HostEvent::Midi([0xB0, 123, 0]), &store);
        assert_eq!(p.active_voices(), 0);
        assert!(render(&mut p, 256).iter().all(|s| *s == 0.0));
    }

    #[test]
    fn param_events_update_store_and_ignore_garbage() {
        let store = ParamStore::default();
        let mut p = processor();
        p.handle(
            HostEvent::ParamValue {
                id: ParamId::Cutoff.raw(),
                value: 500.0,
            },
            &store,
        );
        p.handle(
            HostEvent::ParamValue {
                id: ParamId::Cutoff.raw(),
                value: f64::NAN,
            },
            &store,
        );
        p.handle(
            HostEvent::ParamValue {
                id: 999,
                value: 1.0,
            },
            &store,
        );
        assert_eq!(store.get(ParamId::Cutoff), 500.0);
        render(&mut p, 16);
    }
}
