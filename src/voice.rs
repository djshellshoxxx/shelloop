const MAX_POLYPHONY: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VoiceId {
    pub slot: u16,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VoiceState {
    pub id: VoiceId,
    pub active: bool,
    pub channel: u8,
    pub note: u8,
    pub velocity: f32,
    pub key_held: bool,
    pub sustained: bool,
    started_order: u64,
}

impl VoiceState {
    fn inactive(slot: usize) -> Self {
        Self {
            id: VoiceId {
                slot: slot as u16,
                generation: 0,
            },
            active: false,
            channel: 0,
            note: 0,
            velocity: 0.0,
            key_held: false,
            sustained: false,
            started_order: 0,
        }
    }
}

#[derive(Debug, Clone)]
pub struct VoiceAllocator {
    voices: Vec<VoiceState>,
    sustain: [bool; 16],
    next_generation: u64,
    next_order: u64,
}

impl VoiceAllocator {
    pub fn new(max_polyphony: usize) -> Result<Self, String> {
        if !(1..=MAX_POLYPHONY).contains(&max_polyphony) {
            return Err(format!(
                "polyphony must be between 1 and {MAX_POLYPHONY} voices"
            ));
        }
        let voices = (0..max_polyphony).map(VoiceState::inactive).collect();
        Ok(Self {
            voices,
            sustain: [false; 16],
            next_generation: 1,
            next_order: 1,
        })
    }

    pub fn max_polyphony(&self) -> usize {
        self.voices.len()
    }

    pub fn active_count(&self) -> usize {
        self.voices.iter().filter(|voice| voice.active).count()
    }

    pub fn sustain(&self, channel: u8) -> bool {
        self.sustain[channel.min(15) as usize]
    }

    pub fn voice(&self, id: VoiceId) -> Option<&VoiceState> {
        self.voices
            .get(id.slot as usize)
            .filter(|voice| voice.id.generation == id.generation)
    }

    pub fn is_active(&self, id: VoiceId) -> bool {
        self.voice(id).is_some_and(|voice| voice.active)
    }

    pub fn note_on(&mut self, channel: u8, note: u8, velocity: f32) -> Result<VoiceId, String> {
        if channel > 15 {
            return Err("MIDI channel must be in the zero-based range 0..=15".into());
        }
        if note > 127 {
            return Err("MIDI note must be 0..=127".into());
        }
        if !velocity.is_finite() || !(0.0..=1.0).contains(&velocity) {
            return Err("velocity must be finite and between 0.0 and 1.0".into());
        }

        let slot = self.choose_slot();
        let generation = self.take_generation();
        let order = self.take_order();
        let id = VoiceId {
            slot: slot as u16,
            generation,
        };
        self.voices[slot] = VoiceState {
            id,
            active: true,
            channel,
            note,
            velocity,
            key_held: true,
            sustained: false,
            started_order: order,
        };
        Ok(id)
    }

    pub fn note_off(&mut self, channel: u8, note: u8) -> Option<VoiceId> {
        if channel > 15 || note > 127 {
            return None;
        }
        let slot = self
            .voices
            .iter()
            .enumerate()
            .filter(|(_, voice)| {
                voice.active && voice.key_held && voice.channel == channel && voice.note == note
            })
            .min_by_key(|(_, voice)| voice.started_order)
            .map(|(index, _)| index)?;

        let id = self.voices[slot].id;
        self.voices[slot].key_held = false;
        if self.sustain[channel as usize] {
            self.voices[slot].sustained = true;
            None
        } else {
            self.deactivate(slot);
            Some(id)
        }
    }

    pub fn set_sustain(&mut self, channel: u8, enabled: bool) {
        if channel > 15 {
            return;
        }
        if enabled {
            self.sustain[channel as usize] = true;
        } else {
            self.release_sustain(channel);
        }
    }

    pub fn release_sustain(&mut self, channel: u8) -> usize {
        if channel > 15 {
            return 0;
        }
        self.sustain[channel as usize] = false;
        let mut released = 0;
        for voice in &mut self.voices {
            if voice.active && voice.channel == channel && !voice.key_held && voice.sustained {
                voice.active = false;
                voice.sustained = false;
                released += 1;
            }
        }
        released
    }

    pub fn release_sustain_into(
        &mut self,
        channel: u8,
        released_ids: &mut Vec<VoiceId>,
    ) -> usize {
        released_ids.clear();
        if channel > 15 {
            return 0;
        }

        self.sustain[channel as usize] = false;
        let mut released = 0;
        for voice in &mut self.voices {
            if voice.active && voice.channel == channel && !voice.key_held && voice.sustained {
                let id = voice.id;
                voice.active = false;
                voice.sustained = false;
                if released_ids.len() < released_ids.capacity() {
                    released_ids.push(id);
                }
                released += 1;
            }
        }
        released
    }

    pub fn panic(&mut self) -> usize {
        let active = self.active_count();
        for voice in &mut self.voices {
            voice.active = false;
            voice.key_held = false;
            voice.sustained = false;
        }
        self.sustain = [false; 16];
        active
    }

    fn choose_slot(&self) -> usize {
        if let Some((index, _)) = self
            .voices
            .iter()
            .enumerate()
            .find(|(_, voice)| !voice.active)
        {
            return index;
        }

        self.voices
            .iter()
            .enumerate()
            .filter(|(_, voice)| !voice.key_held)
            .min_by_key(|(_, voice)| voice.started_order)
            .or_else(|| {
                self.voices
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, voice)| voice.started_order)
            })
            .map(|(index, _)| index)
            .expect("allocator always has at least one slot")
    }

    fn deactivate(&mut self, slot: usize) {
        let voice = &mut self.voices[slot];
        voice.active = false;
        voice.key_held = false;
        voice.sustained = false;
    }

    fn take_generation(&mut self) -> u64 {
        let generation = self.next_generation;
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        generation
    }

    fn take_order(&mut self) -> u64 {
        let order = self.next_order;
        self.next_order = self.next_order.wrapping_add(1).max(1);
        order
    }
}
