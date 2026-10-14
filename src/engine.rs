use crate::{
    protect_master, CompiledSynthPatch, Oscillator, SynthPatch, SynthVoice, VoiceAllocator,
    VoiceId, MAX_LFOS,
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
    /// Mod wheel position, 0.0..=1.0 (MIDI CC 1), shared by all voices.
    ModWheel {
        value: f32,
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
    sample_rate: f64,
    bpm: f64,
    /// Shared LFO phase for LFOs with `retrigger` off, so every note joins
    /// the same cycle.
    free_cycles: [f64; MAX_LFOS],
    mod_pan: f32,
    mod_wet: f32,
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
            sample_rate: f64::from(sample_rate),
            bpm: 120.0,
            free_cycles: [0.0; MAX_LFOS],
            mod_pan: 0.0,
            mod_wet: 0.0,
        })
    }

    pub fn patch(&self) -> SynthPatch {
        self.voices[0].patch()
    }

    pub fn apply_patch(&mut self, patch: CompiledSynthPatch) -> bool {
        // Every voice is constructed at the same rate. A mismatch leaves all unchanged.
        if !self.voices[0].apply_patch(patch) {
            return false;
        }
        for voice in &mut self.voices[1..] {
            voice.apply_patch(patch);
        }
        true
    }

    /// Tempo for tempo-synced LFOs.
    pub fn set_tempo(&mut self, bpm: f64) {
        if bpm.is_finite() && bpm > 0.0 {
            self.bpm = bpm;
            for voice in &mut self.voices {
                voice.set_tempo(bpm);
            }
        }
    }

    /// Averaged pan and send-level offsets from the Pan/EffectWet
    /// modulation destinations at the last rendered sample.
    pub fn modulation_pan_and_wet(&self) -> (f32, f32) {
        (self.mod_pan, self.mod_wet)
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
                    self.voices[slot].note_on_with_phase(
                        midi_note_hz(note),
                        velocity,
                        self.free_cycles,
                    );
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
            EngineCommand::ModWheel { value } => {
                for voice in &mut self.voices {
                    voice.set_mod_wheel(value);
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
        let patch_modulation = self.voices[0].patch().modulation;
        if !patch_modulation.is_default() {
            for (index, cycles) in self.free_cycles.iter_mut().enumerate() {
                let lfo = patch_modulation.lfos[index];
                let hz = lfo.sync.map_or(f64::from(lfo.rate_hz), |division| {
                    self.bpm / 60.0 / f64::from(division.beats())
                });
                *cycles += hz / self.sample_rate;
            }
        }
        let mut pan_sum = 0.0;
        let mut wet_sum = 0.0;
        let mut sounding = 0.0;
        let mut mix = 0.0;
        for slot in 0..self.voices.len() {
            let Some(id) = self.voice_ids[slot] else {
                continue;
            };
            let allocator_active = self.allocator.is_active(id);
            let voice_active = self.voices[slot].is_active();

            if allocator_active || voice_active {
                mix += self.voices[slot].next_sample();
                let offsets = self.voices[slot].modulation_offsets();
                pan_sum += offsets.pan;
                wet_sum += offsets.effect_wet;
                sounding += 1.0;
            }

            if !allocator_active && !self.voices[slot].is_active() {
                self.voice_ids[slot] = None;
            }
        }
        if sounding > 0.0 {
            self.mod_pan = pan_sum / sounding;
            self.mod_wet = wet_sum / sounding;
        } else {
            self.mod_pan = 0.0;
            self.mod_wet = 0.0;
        }
        protect_master(mix)
    }
}
