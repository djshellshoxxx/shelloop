#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MidiEvent {
    NoteOn { channel: u8, note: u8, velocity: u8 },
    NoteOff { channel: u8, note: u8, velocity: u8 },
    ControlChange { channel: u8, controller: u8, value: u8 },
    PitchBend { channel: u8, value: i16 },
}

pub fn decode_message(_bytes: &[u8]) -> Result<MidiEvent, String> {
    Err("MIDI decoding not implemented".into())
}

#[derive(Debug, Clone)]
pub struct MidiPerformanceState {
    sustain: [bool; 16],
    pitch_bend: [i16; 16],
}

impl Default for MidiPerformanceState {
    fn default() -> Self {
        Self {
            sustain: [false; 16],
            pitch_bend: [0; 16],
        }
    }
}

impl MidiPerformanceState {
    pub fn apply(&mut self, _event: MidiEvent) {}

    pub fn sustain(&self, channel: u8) -> bool {
        self.sustain[channel.min(15) as usize]
    }

    pub fn pitch_bend(&self, channel: u8) -> i16 {
        self.pitch_bend[channel.min(15) as usize]
    }
}
