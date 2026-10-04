use crate::{protect_master, Oscillator, SynthVoice, VoiceAllocator, VoiceId};

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
}

impl RealtimeSynth {
    pub fn new(sample_rate: f32, oscillator: Oscillator, polyphony: usize) -> Result<Self, String> {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return Err("sample rate must be finite and greater than zero".into());
        }

        let allocator = VoiceAllocator::new(polyphony)?;
        let voices = (0..polyphony)
            .map(|_| SynthVoice::new(sample_rate, oscillator))
            .collect();
        Ok(Self {
            allocator,
            voices,
            voice_ids: vec![None; polyphony],
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
                self.allocator.note_off(channel, note);
            }
            EngineCommand::Sustain { channel, enabled } => {
                self.allocator.set_sustain(channel, enabled);
            }
            EngineCommand::Panic => {
                self.allocator.panic();
            }
        }
    }

    pub fn next_sample(&mut self) -> f32 {
        let mut mix = 0.0;
        for (slot, voice) in self.voices.iter_mut().enumerate() {
            let Some(id) = self.voice_ids[slot] else {
                continue;
            };
            if self.allocator.is_active(id) {
                mix += voice.next_sample();
            }
        }
        protect_master(mix)
    }
}
