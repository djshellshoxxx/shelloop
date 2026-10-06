use crate::{
    protect_master, Oscillator, SynthPatch, SynthVoice, VoiceAllocator, VoiceId,
};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EngineCommand {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: f32,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    Sustain {
        channel: u8,
        enabled: bool,
    },
    Panic,
}

pub fn midi_note_hz(note: u8) -> f32 {
    440.0 * 2.0_f32.powf((f32::from(note) - 69.0) / 12.0)
}

#[derive(Debug, Clone)]
pub struct RealtimeSynth {
    allocator: VoiceAllocator,
    voices: Vec<SynthVoice>,
    voice_ids: Vec<Option<VoiceId>>,
    sustain_releases: Vec<VoiceId>,
}

impl RealtimeSynth {
    pub fn new(sample_rate: f32, oscillator: Oscillator, polyphony: usize) -> Result<Self, String> {
        Self::new_with_patch(sample_rate, SynthPatch::legacy(oscillator), polyphony)
    }

    pub fn new_with_patch(
        sample_rate: f32,
        patch: SynthPatch,
        polyphony: usize,
    ) -> Result<Self, String> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("sample rate must be finite and greater than zero".into());
        }
        patch.validate(sample_rate)?;

        let allocator = VoiceAllocator::new(polyphony)?;
        let voices = (0..polyphony)
            .map(|_| SynthVoice::with_patch(sample_rate, patch))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            allocator,
            voices,
            voice_ids: vec![None; polyphony],
            sustain_releases: Vec::with_capacity(polyphony),
        })
    }

    pub fn active_voice_count(&self) -> usize {
        self.allocator.active_count()
    }

    pub fn handle(&mut self, command: EngineCommand) {
        match command {
            EngineCommand::NoteOn {
                channel,
                note,
                velocity,
            } => {
                if let Ok(id) = self.allocator.note_on(channel, note, velocity) {
                    let slot = usize::from(id.slot);
                    self.voices[slot].note_on(midi_note_hz(note), velocity);
                    self.voice_ids[slot] = Some(id);
                }
            }
            EngineCommand::NoteOff { channel, note } => {
                if let Some(id) = self.allocator.note_off(channel, note) {
                    self.voices[usize::from(id.slot)].note_off();
                }
            }
            EngineCommand::Sustain {
                channel,
                enabled: true,
            } => {
                self.allocator.set_sustain(channel, true);
            }
            EngineCommand::Sustain {
                channel,
                enabled: false,
            } => {
                self.allocator
                    .release_sustain_into(channel, &mut self.sustain_releases);
                for id in self.sustain_releases.iter().copied() {
                    self.voices[usize::from(id.slot)].note_off();
                }
            }
            EngineCommand::Panic => {
                self.allocator.panic();
                for voice in &mut self.voices {
                    voice.force_stop();
                }
            }
        }
    }

    pub fn next_sample(&mut self) -> f32 {
        let mut mix = 0.0;
        for slot in 0..self.voices.len() {
            let Some(id) = self.voice_ids[slot] else {
                continue;
            };
            let allocator_active = self.allocator.is_active(id);
            let voice_active = self.voices[slot].is_active();

            if allocator_active || voice_active {
                mix += self.voices[slot].next_sample();
            }

            if !allocator_active && !self.voices[slot].is_active() {
                self.voice_ids[slot] = None;
            }
        }
        protect_master(mix)
    }
}
