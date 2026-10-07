//! Glue between the full-screen UI building blocks (`tui`) and the session
//! controller. Pure control-thread code: it never touches audio objects,
//! only the controller, telemetry snapshots and UI state.

use crate::controller::{inspector_synth_params, SessionController, TelemetrySnapshot};
use crate::effects::effect_params;
use crate::midi_learn::format_target;
use crate::params::ParamCurve;
use crate::scenes::SceneId;
use crate::tui::{
    playhead_step, CommandHistory, CommandView, GridView, GridViewport, InspectorView,
    RevisionState, StatusLine, StepField, StepView, TrackView, TransportView, TuiSnapshot,
    UiAction, UiMode, COMMAND_WORDS, GRID_VIEWPORT_STEPS,
};
use crate::{
    EffectLocation, EffectParamId, EffectSlotId, GlobalParamId, ParameterTarget, TrackId,
    TrackKind, TrackParamId,
};

const MAX_STATUS_LINES: usize = 64;

/// Effects the runtime must carry out itself (audio-side or I/O).
#[derive(Debug, Clone, PartialEq)]
pub enum UiEffect {
    Quit,
    NoteOn(u8),
    NoteOff(u8),
    Panic,
    BlackBoxSave,
    /// A command line the runtime routes (resample/blackbox) or passes to
    /// the controller.
    Command(String),
    XyPad {
        x: f32,
        y: f32,
    },
}

/// Mutable state of the full-screen UI.
#[derive(Debug, Clone)]
pub struct UiState {
    pub mode: UiMode,
    pub previous_mode: UiMode,
    pub octave: i8,
    pub held: Vec<(char, u8)>,
    pub viewport: GridViewport,
    pub inspector_index: usize,
    pub command: String,
    pub history: CommandHistory,
    pub status: Vec<StatusLine>,
    pub ascii: bool,
}

impl UiState {
    pub fn new(ascii: bool) -> Self {
        Self {
            mode: UiMode::Performance,
            previous_mode: UiMode::Performance,
            octave: 0,
            held: Vec::new(),
            viewport: GridViewport::new(16, GRID_VIEWPORT_STEPS),
            inspector_index: 0,
            command: String::new(),
            history: CommandHistory::default(),
            status: Vec::new(),
            ascii,
        }
    }

    pub fn push_status(&mut self, text: impl Into<String>, error: bool) {
        self.status.push(StatusLine {
            text: text.into(),
            error,
        });
        if self.status.len() > MAX_STATUS_LINES {
            let excess = self.status.len() - MAX_STATUS_LINES;
            self.status.drain(..excess);
        }
    }

    fn set_mode(&mut self, mode: UiMode) {
        if mode != self.mode {
            if !matches!(
                self.mode,
                UiMode::Command | UiMode::Help | UiMode::MidiLearn
            ) {
                self.previous_mode = self.mode;
            }
            self.mode = mode;
        }
    }
}

/// Inspector rows of the selected track: (label, target, value text).
pub fn inspector_targets(controller: &SessionController) -> Vec<ParameterTarget> {
    let track = controller.selected_track();
    let mut targets = vec![
        ParameterTarget::Track {
            track,
            param: TrackParamId::Gain,
        },
        ParameterTarget::Track {
            track,
            param: TrackParamId::Pan,
        },
        ParameterTarget::Track {
            track,
            param: TrackParamId::SendA,
        },
        ParameterTarget::Track {
            track,
            param: TrackParamId::SendB,
        },
    ];
    let definition = controller
        .project()
        .tracks
        .iter()
        .find(|definition| definition.id == track);
    if definition.map(|definition| definition.kind) == Some(TrackKind::Synth) {
        targets
            .extend(inspector_synth_params().map(|param| ParameterTarget::Synth { track, param }));
    }
    if let Some(definition) = definition {
        for (slot, effect) in definition.inserts.iter().enumerate() {
            for (index, descriptor) in effect_params(effect.kind).iter().enumerate() {
                if descriptor.curve != ParamCurve::Discrete {
                    targets.push(ParameterTarget::Effect {
                        location: EffectLocation::Track(track),
                        slot: EffectSlotId(slot as u8),
                        param: EffectParamId(index as u8),
                    });
                }
            }
        }
    }
    targets.push(ParameterTarget::Global(GlobalParamId::MasterGain));
    targets
}

fn adjust_value(
    controller: &SessionController,
    target: ParameterTarget,
    delta: i32,
    coarse: bool,
) -> Option<f32> {
    let descriptor = controller.descriptor(target)?;
    let current = controller.parameter_value(target)?;
    let steps = if coarse { 10.0 } else { 1.0 } * delta as f32;
    let next = match descriptor.curve {
        ParamCurve::Discrete => current + steps,
        curve => {
            let unit = curve.normalize(current, descriptor.min, descriptor.max);
            curve.map(unit + steps / 100.0, descriptor.min, descriptor.max)
        }
    };
    Some(next.clamp(descriptor.min, descriptor.max))
}

/// Apply one routed UI action. Controller errors become status lines.
pub fn handle_action(
    action: UiAction,
    ui: &mut UiState,
    controller: &mut SessionController,
    current_frame: u64,
) -> Vec<UiEffect> {
    let mut effects = Vec::new();
    let report = |ui: &mut UiState, result: Result<String, String>| match result {
        Ok(message) if !message.is_empty() => ui.push_status(message, false),
        Ok(_) => {}
        Err(error) => ui.push_status(error, true),
    };
    let track = controller.selected_track();
    match action {
        UiAction::SetMode(mode) => ui.set_mode(mode),
        UiAction::Quit => effects.push(UiEffect::Quit),
        UiAction::ClearStatus => ui.status.clear(),
        UiAction::Resize { .. } => {}
        UiAction::NoteOn { key, offset } => {
            if !ui.held.iter().any(|(held, _)| *held == key) {
                let note = (60 + i16::from(ui.octave) * 12 + i16::from(offset)).clamp(0, 127) as u8;
                ui.held.push((key, note));
                effects.push(UiEffect::NoteOn(note));
            }
        }
        UiAction::NoteOff { key, .. } => {
            if let Some(index) = ui.held.iter().position(|(held, _)| *held == key) {
                let (_, note) = ui.held.remove(index);
                effects.push(UiEffect::NoteOff(note));
            }
        }
        UiAction::OctaveDown => ui.octave = crate::shift_octave(ui.octave, -1),
        UiAction::OctaveUp => ui.octave = crate::shift_octave(ui.octave, 1),
        UiAction::Panic => {
            ui.held.clear();
            effects.push(UiEffect::Panic);
        }
        UiAction::TogglePlay => report(ui, controller.execute("play", current_frame)),
        UiAction::Restart => report(ui, controller.execute("restart", current_frame)),
        UiAction::SelectTrack(delta) => {
            let ids: Vec<TrackId> = controller.project().tracks.iter().map(|t| t.id).collect();
            if let Some(position) = ids.iter().position(|id| *id == track) {
                let next = (position as i32 + delta).rem_euclid(ids.len() as i32) as usize;
                report(
                    ui,
                    controller.select_track(ids[next]).map(|()| String::new()),
                );
            }
        }
        UiAction::SelectTrackIndex(index) => {
            if let Some(id) = controller.project().tracks.get(index).map(|t| t.id) {
                report(ui, controller.select_track(id).map(|()| String::new()));
            }
        }
        UiAction::ToggleMute(id) | UiAction::ToggleSolo(id) => {
            let param = if matches!(action, UiAction::ToggleMute(_)) {
                TrackParamId::Mute
            } else {
                TrackParamId::Solo
            };
            let target = ParameterTarget::Track { track: id, param };
            let current = controller.parameter_value(target).unwrap_or(0.0);
            report(ui, controller.set_parameter(target, 1.0 - current));
        }
        UiAction::ToggleMuteSelected => report(ui, controller.execute("mute", current_frame)),
        UiAction::ToggleSoloSelected => report(ui, controller.execute("solo", current_frame)),
        UiAction::LaunchSceneSlot(slot) => {
            match controller.project().scenes.get(slot).map(|scene| scene.id) {
                Some(id) => report(ui, controller.launch_scene(id, None, current_frame)),
                None => ui.push_status(format!("no scene in slot {}", slot + 1), true),
            }
        }
        UiAction::SceneNext => report(ui, controller.execute("scene next", current_frame)),
        UiAction::ScenePrev => report(ui, controller.execute("scene prev", current_frame)),
        UiAction::BlackBoxSave => effects.push(UiEffect::BlackBoxSave),
        UiAction::CursorMove(delta) => {
            ui.viewport.move_cursor(delta);
            controller.set_selected_step(ui.viewport.cursor);
        }
        UiAction::CursorTo(index) => {
            ui.viewport.set_cursor(index);
            controller.set_selected_step(ui.viewport.cursor);
        }
        UiAction::CursorHome => {
            ui.viewport.set_cursor(0);
            controller.set_selected_step(0);
        }
        UiAction::CursorEnd => {
            ui.viewport.set_cursor(ui.viewport.len.saturating_sub(1));
            controller.set_selected_step(ui.viewport.cursor);
        }
        UiAction::ToggleStep => {
            controller.set_selected_step(ui.viewport.cursor);
            report(ui, controller.toggle_selected_step(current_frame));
        }
        UiAction::StepField { field, delta } => {
            let step = ui.viewport.cursor;
            let current = controller
                .editors()
                .editor(track)
                .and_then(|editor| editor.pattern().steps.get(step).copied().flatten());
            let Some(current) = current else {
                ui.push_status("step is empty; toggle it first", true);
                return effects;
            };
            let command = match field {
                StepField::Note => format!(
                    "step {} note {}",
                    step + 1,
                    (i32::from(current.note) + delta).clamp(0, 127)
                ),
                StepField::Velocity => format!(
                    "step {} vel {}",
                    step + 1,
                    (current.velocity + 0.05 * delta as f32).clamp(0.0, 1.0)
                ),
                StepField::Gate => format!(
                    "step {} gate {}",
                    step + 1,
                    (current.gate + 0.05 * delta as f32).clamp(0.0, 1.0)
                ),
                StepField::Probability => format!(
                    "step {} prob {}",
                    step + 1,
                    (current.probability + 0.05 * delta as f32).clamp(0.0, 1.0)
                ),
                StepField::Ratchets => format!(
                    "step {} ratchet {}",
                    step + 1,
                    (i32::from(current.ratchets) + delta).clamp(1, 8)
                ),
                StepField::Microtiming => format!(
                    "step {} micro {}",
                    step + 1,
                    current.microtiming_frames + delta * 48
                ),
            };
            report(ui, controller.execute(&command, current_frame));
        }
        UiAction::ScrollGrid(delta) => ui.viewport.scroll(delta),
        UiAction::InspectorMove(delta) => {
            let len = inspector_targets(controller).len().max(1);
            ui.inspector_index =
                (ui.inspector_index as i32 + delta).rem_euclid(len as i32) as usize;
        }
        UiAction::InspectorAdjust { delta, coarse } => {
            let targets = inspector_targets(controller);
            if let Some(target) = targets.get(ui.inspector_index).copied() {
                if let Some(value) = adjust_value(controller, target, delta, coarse) {
                    report(ui, controller.set_parameter(target, value));
                }
            }
        }
        UiAction::LearnSelectedParameter => {
            let targets = inspector_targets(controller);
            if let Some(target) = targets.get(ui.inspector_index).copied() {
                let line = format!("learn {}", format_target(target));
                report(ui, controller.execute(&line, current_frame));
                ui.set_mode(UiMode::MidiLearn);
            }
        }
        UiAction::ConfirmLearn => {
            report(ui, controller.execute("learn confirm", current_frame));
            ui.mode = ui.previous_mode;
        }
        UiAction::CancelLearn => {
            report(ui, controller.execute("learn cancel", current_frame));
            ui.mode = ui.previous_mode;
        }
        UiAction::XyPad { x, y } => effects.push(UiEffect::XyPad { x, y }),
        UiAction::XyPadRelease => {}
        UiAction::CommandInput(character) => ui.command.push(character),
        UiAction::CommandBackspace => {
            ui.command.pop();
        }
        UiAction::CommandSubmit => {
            let line = std::mem::take(&mut ui.command);
            if !line.trim().is_empty() {
                ui.history.push(&line);
                effects.push(UiEffect::Command(line));
            }
            ui.mode = ui.previous_mode;
        }
        UiAction::CommandCancel => {
            ui.command.clear();
            ui.mode = ui.previous_mode;
        }
        UiAction::CommandHistory(delta) => {
            let entry = if delta < 0 {
                ui.history.prev()
            } else {
                ui.history.next()
            };
            if let Some(entry) = entry {
                ui.command = entry.to_string();
            }
        }
        UiAction::CommandComplete => {
            if let Some(completed) = crate::tui::complete_command(&ui.command, COMMAND_WORDS) {
                ui.command = completed;
            }
        }
    }
    effects
}

/// Extra runtime status shown in the transport bar.
#[derive(Debug, Clone, Default)]
pub struct RuntimeStatus {
    pub audio_device: String,
    pub sample_rate: u32,
    pub midi: Option<String>,
    pub recording: Option<String>,
    pub black_box: Option<String>,
    pub resample: Option<String>,
}

/// Build the immutable snapshot the renderer draws.
pub fn build_snapshot(
    controller: &SessionController,
    telemetry: &TelemetrySnapshot,
    ui: &mut UiState,
    runtime: &RuntimeStatus,
) -> TuiSnapshot {
    let project = controller.project();
    let sample_rate = controller.sample_rate().max(1);
    let frames_per_beat = f64::from(sample_rate) * 60.0 / project.bpm;
    let frames_per_step = frames_per_beat / f64::from(project.steps_per_beat.max(1));
    let beat_index = (telemetry.position_frame as f64 / frames_per_beat) as u64;
    let scene_name = |id: SceneId| {
        project
            .scenes
            .iter()
            .find(|scene| scene.id == id)
            .map_or_else(|| format!("#{}", id.0), |scene| scene.name.clone())
    };
    let frames_per_bar = frames_per_beat * 4.0;
    let transport = TransportView {
        playing: telemetry.playing,
        bpm: project.bpm,
        bar: beat_index / 4 + 1,
        beat: (beat_index % 4) as u32 + 1,
        step_in_beat: ((telemetry.position_frame as f64 / frames_per_step) as u64
            % u64::from(project.steps_per_beat.max(1))) as u32
            + 1,
        active_scene: telemetry.active_scene.map(scene_name),
        queued_scene: telemetry
            .queued_scene
            .map(|(id, frame)| (scene_name(id), (frame as f64 / frames_per_bar) as u64 + 1)),
        chain: telemetry.chain.map(|(id, step, paused)| {
            let name = project
                .chains
                .iter()
                .find(|chain| chain.id == id)
                .map_or_else(|| format!("#{}", id.0), |chain| chain.name.clone());
            format!(
                "{name} step {}{}",
                step + 1,
                if paused { " (paused)" } else { "" }
            )
        }),
        recording: runtime.recording.clone(),
        black_box: runtime.black_box.clone(),
        midi: runtime.midi.clone(),
        audio_device: runtime.audio_device.clone(),
        sample_rate: runtime.sample_rate,
        resample: runtime.resample.clone(),
    };

    let tracks: Vec<TrackView> = project
        .tracks
        .iter()
        .map(|definition| {
            let live = telemetry
                .tracks
                .iter()
                .find(|track| track.id == definition.id);
            let pattern_id = live.map_or(crate::PRIMARY_PATTERN_ID, |live| live.active_pattern);
            let pattern_label = definition
                .pattern_by_id(pattern_id)
                .map_or_else(String::new, |pattern| pattern.name.clone());
            let revision_state = match live {
                Some(live) => match live.queued_revision {
                    Some(queued) => RevisionState::Queued {
                        active: live.active_revision,
                        queued,
                    },
                    None => RevisionState::Active(live.active_revision),
                },
                None => RevisionState::Active(0),
            };
            TrackView {
                id: definition.id,
                name: definition.name.clone(),
                kind: definition.kind,
                muted: live.map_or(definition.muted, |live| live.muted),
                soloed: live.map_or(definition.soloed, |live| live.soloed),
                audible: live.is_none_or(|live| live.audible),
                gain: definition.gain,
                pan: definition.pan,
                peak: live.map_or(0.0, |live| live.peak),
                pattern_label,
                revision_state,
            }
        })
        .collect();
    let selected_track = controller.selected_track();
    let selected_index = project
        .tracks
        .iter()
        .position(|track| track.id == selected_track)
        .unwrap_or(0);

    let pattern = controller
        .editors()
        .editor(selected_track)
        .map(|editor| editor.pattern().clone());
    let (steps, lock_counts) = match &pattern {
        Some(pattern) => (
            pattern
                .steps
                .iter()
                .map(|step| step.as_ref().map(StepView::from))
                .collect(),
            (0..pattern.len())
                .map(|index| pattern.step_locks(index).len() as u8)
                .collect(),
        ),
        None => (Vec::new(), Vec::new()),
    };
    let len = steps.len();
    if ui.viewport.len != len {
        ui.viewport.set_len(len);
    }
    let playhead = playhead_step(telemetry.position_frame, frames_per_step, len);

    let targets = inspector_targets(controller);
    ui.inspector_index = ui.inspector_index.min(targets.len().saturating_sub(1));
    let mut rows: Vec<(String, String)> = targets
        .iter()
        .map(|target| {
            let value = controller
                .parameter_value(*target)
                .map_or_else(|| "-".into(), |value| format!("{value:.3}"));
            (format_target(*target), value)
        })
        .collect();
    if let Some(step) = pattern
        .as_ref()
        .and_then(|pattern| pattern.steps.get(ui.viewport.cursor).copied().flatten())
    {
        rows.insert(
            0,
            (
                format!("step {}", ui.viewport.cursor + 1),
                format!(
                    "note {} vel {:.2} gate {:.2} prob {:.2} x{} {:+}f",
                    step.note,
                    step.velocity,
                    step.gate,
                    step.probability,
                    step.ratchets,
                    step.microtiming_frames
                ),
            ),
        );
    }
    let selected_row = if rows.len() > targets.len() {
        ui.inspector_index + 1
    } else {
        ui.inspector_index
    };
    let kind = project
        .tracks
        .get(selected_index)
        .map_or("", |track| match track.kind {
            TrackKind::Synth => "synth",
            TrackKind::Sample => "sample",
            TrackKind::ExternalMidi => "midi",
        });

    TuiSnapshot {
        transport,
        tracks,
        selected_track: selected_index,
        grid: GridView {
            steps,
            lock_counts,
            viewport: ui.viewport,
            playhead,
        },
        inspector: InspectorView {
            title: format!("track {} {kind}", selected_track.0),
            rows,
            selected: selected_row,
        },
        status: ui.status.clone(),
        command: (ui.mode == UiMode::Command).then(|| CommandView {
            buffer: ui.command.clone(),
            completion: crate::tui::complete_command(&ui.command, COMMAND_WORDS),
        }),
        mode: ui.mode,
        learn: controller.learning(),
        ascii: ui.ascii,
    }
}
