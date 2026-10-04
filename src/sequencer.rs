use crate::{EngineCommand, Pattern, PatternEvent, PatternScheduler};

const EVENT_BUFFER_CAPACITY: usize = 4096;
const PENDING_NOTE_OFF_CAPACITY: usize = 8192;
const CACHE_FRAMES: u32 = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingNoteOff {
    frame: u64,
    channel: u8,
    note: u8,
}

#[derive(Debug, Clone)]
pub struct LiveSequencer {
    scheduler: PatternScheduler,
    pattern: Pattern,
    position_frame: u64,
    playing: bool,
    cache_start: u64,
    cache_end: u64,
    event_buffer: Vec<PatternEvent>,
    event_index: usize,
    pending_note_offs: Vec<PendingNoteOff>,
}

impl LiveSequencer {
    pub const MAX_COMMANDS_PER_FRAME: usize = EVENT_BUFFER_CAPACITY + PENDING_NOTE_OFF_CAPACITY;

    pub fn new(
        sample_rate: u32,
        bpm: f64,
        steps_per_beat: u32,
        project_seed: u64,
        pattern: Pattern,
    ) -> Result<Self, String> {
        pattern.validate()?;
        Ok(Self {
            scheduler: PatternScheduler::new(sample_rate, bpm, steps_per_beat, project_seed)?,
            pattern,
            position_frame: 0,
            playing: true,
            cache_start: 0,
            cache_end: 0,
            event_buffer: Vec::with_capacity(EVENT_BUFFER_CAPACITY),
            event_index: 0,
            pending_note_offs: Vec::with_capacity(PENDING_NOTE_OFF_CAPACITY),
        })
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn position_frame(&self) -> u64 {
        self.position_frame
    }

    pub fn set_playing(&mut self, playing: bool) {
        if self.playing && !playing {
            self.pending_note_offs.clear();
        }
        self.playing = playing;
    }

    pub fn toggle_playing(&mut self) -> bool {
        self.set_playing(!self.playing);
        self.playing
    }

    pub fn restart(&mut self) {
        self.position_frame = 0;
        self.playing = true;
        self.cache_start = 0;
        self.cache_end = 0;
        self.event_buffer.clear();
        self.event_index = 0;
        self.pending_note_offs.clear();
    }

    pub fn fill_commands(&mut self, output: &mut Vec<EngineCommand>) {
        output.clear();
        if !self.playing {
            return;
        }

        self.ensure_cache();
        self.emit_note_offs(output);
        self.emit_note_ons(output);
        self.position_frame = self.position_frame.saturating_add(1);
    }

    fn ensure_cache(&mut self) {
        if self.position_frame >= self.cache_start && self.position_frame < self.cache_end {
            return;
        }

        self.cache_start =
            (self.position_frame / u64::from(CACHE_FRAMES)) * u64::from(CACHE_FRAMES);
        self.cache_end = self.cache_start.saturating_add(u64::from(CACHE_FRAMES));
        self.scheduler.schedule_block_into(
            &self.pattern,
            self.cache_start,
            CACHE_FRAMES,
            &mut self.event_buffer,
        );
        self.event_index = self
            .event_buffer
            .partition_point(|event| event.absolute_frame < self.position_frame);
    }

    fn emit_note_offs(&mut self, output: &mut Vec<EngineCommand>) {
        let mut index = self.pending_note_offs.len();
        while index > 0 {
            index -= 1;
            let pending = self.pending_note_offs[index];
            if pending.frame <= self.position_frame {
                if output.len() < Self::MAX_COMMANDS_PER_FRAME {
                    output.push(EngineCommand::NoteOff {
                        channel: pending.channel,
                        note: pending.note,
                    });
                }
                self.pending_note_offs.swap_remove(index);
            }
        }
    }

    fn emit_note_ons(&mut self, output: &mut Vec<EngineCommand>) {
        while let Some(event) = self.event_buffer.get(self.event_index) {
            if event.absolute_frame != self.position_frame {
                break;
            }

            if output.len() < Self::MAX_COMMANDS_PER_FRAME {
                output.push(EngineCommand::NoteOn {
                    channel: event.channel,
                    note: event.note,
                    velocity: event.velocity,
                });
            }

            if self.pending_note_offs.len() < PENDING_NOTE_OFF_CAPACITY {
                self.pending_note_offs.push(PendingNoteOff {
                    frame: event
                        .absolute_frame
                        .saturating_add(u64::from(event.duration_frames)),
                    channel: event.channel,
                    note: event.note,
                });
            }
            self.event_index += 1;
        }
    }
}
