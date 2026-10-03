#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MidiEvent {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    PitchBend {
        channel: u8,
        value: i16,
    },
}

pub fn decode_message(bytes: &[u8]) -> Result<MidiEvent, String> {
    if bytes.len() != 3 {
        return Err("expected a three-byte MIDI channel voice message".into());
    }

    let status = bytes[0];
    if status < 0x80 || status >= 0xF0 {
        return Err("unsupported MIDI status byte".into());
    }
    if bytes[1] > 0x7F || bytes[2] > 0x7F {
        return Err("MIDI data bytes must be seven-bit values".into());
    }

    let channel = status & 0x0F;
    match status & 0xF0 {
        0x80 => Ok(MidiEvent::NoteOff {
            channel,
            note: bytes[1],
            velocity: bytes[2],
        }),
        0x90 if bytes[2] == 0 => Ok(MidiEvent::NoteOff {
            channel,
            note: bytes[1],
            velocity: 0,
        }),
        0x90 => Ok(MidiEvent::NoteOn {
            channel,
            note: bytes[1],
            velocity: bytes[2],
        }),
        0xB0 => Ok(MidiEvent::ControlChange {
            channel,
            controller: bytes[1],
            value: bytes[2],
        }),
        0xE0 => {
            let unsigned = ((bytes[2] as i16) << 7) | bytes[1] as i16;
            Ok(MidiEvent::PitchBend {
                channel,
                value: unsigned - 8192,
            })
        }
        _ => Err("unsupported MIDI channel voice message".into()),
    }
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
    pub fn apply(&mut self, event: MidiEvent) {
        match event {
            MidiEvent::ControlChange {
                channel,
                controller: 64,
                value,
            } => self.sustain[channel.min(15) as usize] = value >= 64,
            MidiEvent::PitchBend { channel, value } => {
                self.pitch_bend[channel.min(15) as usize] = value.clamp(-8192, 8191)
            }
            _ => {}
        }
    }

    pub fn sustain(&self, channel: u8) -> bool {
        self.sustain[channel.min(15) as usize]
    }

    pub fn pitch_bend(&self, channel: u8) -> i16 {
        self.pitch_bend[channel.min(15) as usize]
    }
}
