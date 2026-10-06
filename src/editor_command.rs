use crate::{
    compile_pattern_revision, CompiledPatternRevision, Pattern, PatternEditor, PatternStep,
    QuantizeBoundary, QuantizedChange, TrackId, MAX_PATTERN_STEPS, MAX_REALTIME_TRACKS,
};

const MAX_MICROTIMING_FRAMES: i32 = 192_000;
const MAX_RATCHETS: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StepEdit {
    Toggle,
    Clear,
    Note(u8),
    Velocity(f32),
    Gate(f32),
    Probability(f32),
    Ratchets(u8),
    Microtiming(i32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatternEditCommand {
    SelectTrack(TrackId),
    Step { index: usize, edit: StepEdit },
    Length(usize),
    Swing(f32),
    RotateLeft(usize),
    RotateRight(usize),
    Undo,
    Redo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditOutcome {
    pub track: TrackId,
    pub revision: Option<u64>,
    pub changed: bool,
}

#[derive(Debug, Clone)]
struct TrackPatternEditor {
    id: TrackId,
    editor: PatternEditor,
}

#[derive(Debug, Clone)]
pub struct ProjectPatternEditors {
    tracks: Vec<TrackPatternEditor>,
    selected_track: TrackId,
}

impl ProjectPatternEditors {
    pub fn new(
        tracks: impl IntoIterator<Item = (TrackId, Pattern)>,
        history_capacity: usize,
    ) -> Result<Self, String> {
        let mut editors = Vec::new();
        for (id, pattern) in tracks {
            if editors.iter().any(|entry: &TrackPatternEditor| entry.id == id) {
                return Err(format!("duplicate pattern editor track id: {}", id.0));
            }
            if editors.len() >= MAX_REALTIME_TRACKS {
                return Err(format!(
                    "pattern editor supports at most {MAX_REALTIME_TRACKS} tracks"
                ));
            }
            editors.push(TrackPatternEditor {
                id,
                editor: PatternEditor::new(pattern, history_capacity)?,
            });
        }

        let selected_track = editors
            .first()
            .map(|entry| entry.id)
            .ok_or_else(|| "pattern editor requires at least one track".to_string())?;

        Ok(Self {
            tracks: editors,
            selected_track,
        })
    }

    pub fn selected_track(&self) -> TrackId {
        self.selected_track
    }

    pub fn editor(&self, track: TrackId) -> Option<&PatternEditor> {
        self.tracks
            .iter()
            .find(|entry| entry.id == track)
            .map(|entry| &entry.editor)
    }

    pub fn apply(&mut self, command: PatternEditCommand) -> Result<EditOutcome, String> {
        if let PatternEditCommand::SelectTrack(track) = command {
            if self.editor(track).is_none() {
                return Err(format!("track id {} does not have a pattern editor", track.0));
            }
            self.selected_track = track;
            return Ok(EditOutcome {
                track,
                revision: None,
                changed: false,
            });
        }

        let track = self.selected_track;
        let editor = self.editor_mut(track)?;
        let revision = match command {
            PatternEditCommand::SelectTrack(_) => unreachable!(),
            PatternEditCommand::Step { index, edit } => Some(apply_step_edit(editor, index, edit)?),
            PatternEditCommand::Length(len) => Some(editor.set_length(len)?),
            PatternEditCommand::Swing(swing) => Some(editor.set_swing(swing)?),
            PatternEditCommand::RotateLeft(amount) => Some(editor.rotate_left(amount)?),
            PatternEditCommand::RotateRight(amount) => Some(editor.rotate_right(amount)?),
            PatternEditCommand::Undo => editor.undo().then(|| editor.revision()),
            PatternEditCommand::Redo => editor.redo().then(|| editor.revision()),
        };

        Ok(EditOutcome {
            track,
            revision,
            changed: revision.is_some(),
        })
    }

    pub fn queue_selected_revision(
        &self,
        current_frame: u64,
        sample_rate: u32,
        bpm: f64,
        steps_per_beat: u32,
        boundary: QuantizeBoundary,
    ) -> Result<(TrackId, QuantizedChange<CompiledPatternRevision>), String> {
        let editor = self
            .editor(self.selected_track)
            .ok_or_else(|| "selected track editor is unavailable".to_string())?;
        let queued =
            editor.queue_revision(current_frame, sample_rate, bpm, steps_per_beat, boundary)?;
        Ok((self.selected_track, compile_pattern_revision(&queued)?))
    }

    fn editor_mut(&mut self, track: TrackId) -> Result<&mut PatternEditor, String> {
        self.tracks
            .iter_mut()
            .find(|entry| entry.id == track)
            .map(|entry| &mut entry.editor)
            .ok_or_else(|| format!("track id {} does not have a pattern editor", track.0))
    }
}

pub fn parse_pattern_edit_command(line: &str) -> Result<PatternEditCommand, String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    let Some(verb) = parts.first().copied() else {
        return Err("empty pattern edit command".into());
    };

    match verb.to_ascii_lowercase().as_str() {
        "track" => {
            require_len(&parts, 2, "track requires an id")?;
            let id = parse_u16(parts[1], "track id")?;
            Ok(PatternEditCommand::SelectTrack(TrackId(id)))
        }
        "step" => parse_step_command(&parts),
        "length" => {
            require_len(&parts, 2, "length requires a step count")?;
            let len = parse_usize(parts[1], "pattern length")?;
            if !(1..=MAX_PATTERN_STEPS).contains(&len) {
                return Err(format!(
                    "pattern length must be between 1 and {MAX_PATTERN_STEPS}"
                ));
            }
            Ok(PatternEditCommand::Length(len))
        }
        "swing" => {
            require_len(&parts, 2, "swing requires a value")?;
            let value = parse_f32(parts[1], "swing")?;
            if !(0.0..=0.75).contains(&value) {
                return Err("swing must be between 0.0 and 0.75".into());
            }
            Ok(PatternEditCommand::Swing(value))
        }
        "rotate" => {
            require_len(&parts, 3, "rotate requires left/right and an amount")?;
            let amount = parse_usize(parts[2], "rotate amount")?;
            match parts[1].to_ascii_lowercase().as_str() {
                "left" => Ok(PatternEditCommand::RotateLeft(amount)),
                "right" => Ok(PatternEditCommand::RotateRight(amount)),
                _ => Err("rotate direction must be left or right".into()),
            }
        }
        "undo" => {
            require_len(&parts, 1, "undo does not accept arguments")?;
            Ok(PatternEditCommand::Undo)
        }
        "redo" => {
            require_len(&parts, 1, "redo does not accept arguments")?;
            Ok(PatternEditCommand::Redo)
        }
        _ => Err(format!("unknown pattern edit command: {verb}")),
    }
}

fn parse_step_command(parts: &[&str]) -> Result<PatternEditCommand, String> {
    if parts.len() < 3 {
        return Err("step requires a one-based step number and operation".into());
    }
    let one_based = parse_usize(parts[1], "step number")?;
    if !(1..=MAX_PATTERN_STEPS).contains(&one_based) {
        return Err(format!(
            "step number must be between 1 and {MAX_PATTERN_STEPS}"
        ));
    }
    let index = one_based - 1;
    let operation = parts[2].to_ascii_lowercase();

    let edit = match operation.as_str() {
        "toggle" => {
            require_len(parts, 3, "step toggle does not accept a value")?;
            StepEdit::Toggle
        }
        "clear" => {
            require_len(parts, 3, "step clear does not accept a value")?;
            StepEdit::Clear
        }
        "note" => {
            require_len(parts, 4, "step note requires a MIDI note")?;
            let note = parse_u8(parts[3], "MIDI note")?;
            if note > 127 {
                return Err("MIDI note must be between 0 and 127".into());
            }
            StepEdit::Note(note)
        }
        "velocity" => {
            require_len(parts, 4, "step velocity requires a value")?;
            StepEdit::Velocity(parse_unit(parts[3], "velocity")?)
        }
        "gate" => {
            require_len(parts, 4, "step gate requires a value")?;
            StepEdit::Gate(parse_unit(parts[3], "gate")?)
        }
        "probability" => {
            require_len(parts, 4, "step probability requires a value")?;
            StepEdit::Probability(parse_unit(parts[3], "probability")?)
        }
        "ratchets" => {
            require_len(parts, 4, "step ratchets requires a count")?;
            let ratchets = parse_u8(parts[3], "ratchets")?;
            if !(1..=MAX_RATCHETS).contains(&ratchets) {
                return Err(format!(
                    "ratchets must be between 1 and {MAX_RATCHETS}"
                ));
            }
            StepEdit::Ratchets(ratchets)
        }
        "micro" | "microtiming" => {
            require_len(parts, 4, "step microtiming requires a frame offset")?;
            let frames = parts[3]
                .parse::<i32>()
                .map_err(|_| "microtiming must be an integer frame offset".to_string())?;
            if frames.unsigned_abs() > MAX_MICROTIMING_FRAMES as u32 {
                return Err(format!(
                    "microtiming must be between -{MAX_MICROTIMING_FRAMES} and {MAX_MICROTIMING_FRAMES}"
                ));
            }
            StepEdit::Microtiming(frames)
        }
        _ => return Err(format!("unknown step edit operation: {}", parts[2])),
    };

    Ok(PatternEditCommand::Step { index, edit })
}

fn apply_step_edit(
    editor: &mut PatternEditor,
    index: usize,
    edit: StepEdit,
) -> Result<u64, String> {
    if index >= editor.pattern().steps.len() {
        return Err(format!(
            "step {} is outside pattern length {}",
            index + 1,
            editor.pattern().steps.len()
        ));
    }

    match edit {
        StepEdit::Toggle => editor.toggle_step(index, default_step()),
        StepEdit::Clear => editor.clear_step(index),
        StepEdit::Note(note) => {
            let mut step = editor.pattern().steps[index].unwrap_or_else(default_step);
            step.note = note;
            editor.set_step(index, Some(step))
        }
        StepEdit::Velocity(velocity) => {
            let mut step = editor.pattern().steps[index].unwrap_or_else(default_step);
            step.velocity = velocity;
            editor.set_step(index, Some(step))
        }
        StepEdit::Gate(gate) => {
            let mut step = editor.pattern().steps[index].unwrap_or_else(default_step);
            step.gate = gate;
            editor.set_step(index, Some(step))
        }
        StepEdit::Probability(probability) => {
            let mut step = editor.pattern().steps[index].unwrap_or_else(default_step);
            step.probability = probability;
            editor.set_step(index, Some(step))
        }
        StepEdit::Ratchets(ratchets) => {
            let mut step = editor.pattern().steps[index].unwrap_or_else(default_step);
            step.ratchets = ratchets;
            editor.set_step(index, Some(step))
        }
        StepEdit::Microtiming(frames) => {
            let mut step = editor.pattern().steps[index].unwrap_or_else(default_step);
            step.microtiming_frames = frames;
            editor.set_step(index, Some(step))
        }
    }
}

fn default_step() -> PatternStep {
    PatternStep {
        note: 60,
        velocity: 0.8,
        gate: 0.5,
        probability: 1.0,
        ratchets: 1,
        microtiming_frames: 0,
    }
}

fn require_len(parts: &[&str], expected: usize, message: &str) -> Result<(), String> {
    if parts.len() == expected {
        Ok(())
    } else {
        Err(message.into())
    }
}

fn parse_u16(value: &str, label: &str) -> Result<u16, String> {
    value
        .parse::<u16>()
        .map_err(|_| format!("{label} must be a non-negative integer"))
}

fn parse_u8(value: &str, label: &str) -> Result<u8, String> {
    value
        .parse::<u8>()
        .map_err(|_| format!("{label} must be an integer"))
}

fn parse_usize(value: &str, label: &str) -> Result<usize, String> {
    value
        .parse::<usize>()
        .map_err(|_| format!("{label} must be a non-negative integer"))
}

fn parse_f32(value: &str, label: &str) -> Result<f32, String> {
    let parsed = value
        .parse::<f32>()
        .map_err(|_| format!("{label} must be numeric"))?;
    if !parsed.is_finite() {
        return Err(format!("{label} must be finite"));
    }
    Ok(parsed)
}

fn parse_unit(value: &str, label: &str) -> Result<f32, String> {
    let parsed = parse_f32(value, label)?;
    if !(0.0..=1.0).contains(&parsed) {
        return Err(format!("{label} must be between 0.0 and 1.0"));
    }
    Ok(parsed)
}
