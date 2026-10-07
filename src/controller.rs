//! Session controller: the control-thread model that turns text commands,
//! MIDI-learned controls and full-screen UI actions into bounded typed
//! messages for the audio thread. It owns every piece of control-side state
//! (pattern editors, base synth patches, effect settings, scenes, MIDI
//! mappings, variation proposals) so the audio callback only ever receives
//! validated, compiled data.

use crate::effects::{effect_params, find_effect_param, EffectKind};
use crate::midi_learn::{
    format_target, parse_learn_command, ConflictPolicy, LearnCommand, LearnProgress, LearnState,
    MappedOutput, MidiLearn, TargetResolver,
};
use crate::params::{
    parse_synth_param, sample_param_descriptor, synth_param_descriptor, track_param_descriptor,
    MASTER_GAIN,
};
use crate::scenes::{resolve_chain, resolve_scene, ChainId, PatternId, SceneId};
use crate::variation::{
    apply_lock_command, format_locks, format_proposal, parse_variation_command, VariationCommand,
    VariationRequest, VariationSession,
};
use crate::{
    parse_pattern_edit_command, parse_synth_parameter_command, ActionId, ChainStatus,
    CompiledPatternRevision, CompiledSamplePlayback, CompiledSynthPatch, EffectLocation,
    EffectParamId, EffectSlotId, GlobalParamId, LockTarget, MidiEvent, MultiTrackEngine,
    MultiTrackProject, ParamDescriptor, ParameterLock, ParameterTarget, PreparedTrack,
    ProjectPatternEditors, QuantizeBoundary, QuantizedChange, SampleParamId, SynthParamId,
    SynthParamValue, SynthPatch, TrackCommand, TrackId, TrackKind, TrackParamId,
    MAX_LOCKS_PER_STEP, MAX_REALTIME_TRACKS,
};
use crossbeam_channel::{Sender, TrySendError};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

pub const AUDIO_MESSAGE_QUEUE_CAPACITY: usize = 32;
pub const AUDIO_MESSAGES_PER_FRAME: usize = 2;
pub const EDIT_HISTORY_CAPACITY: usize = 128;
const BEATS_PER_BAR: u32 = 4;

/// Bounded control → audio message. Large variants carry compiled data so
/// the callback never parses, validates or allocates.
#[allow(clippy::large_enum_variant)]
pub enum AudioMessage {
    Revision {
        track: TrackId,
        pattern: PatternId,
        change: QuantizedChange<CompiledPatternRevision>,
    },
    SynthPatch {
        track: TrackId,
        patch: CompiledSynthPatch,
    },
    Track(TrackCommand),
    MasterGain(f32),
    EffectParam {
        location: EffectLocation,
        slot: EffectSlotId,
        param: EffectParamId,
        value: f32,
    },
    EffectBypass {
        location: EffectLocation,
        slot: EffectSlotId,
        bypassed: bool,
    },
    LaunchScene {
        scene: SceneId,
        frame: u64,
    },
    StartChain {
        chain: ChainId,
        frame: u64,
    },
    StopChain,
    PauseChain(bool),
    TogglePlay,
    Restart,
    Panic,
    AddTrack(PreparedTrack),
    ReplaceSample {
        track: TrackId,
        playback: CompiledSamplePlayback,
    },
}

/// Heap-owning values handed back by the audio thread so they are freed on
/// the control thread.
#[allow(clippy::large_enum_variant)]
pub enum Garbage {
    Track(PreparedTrack),
    Playback(CompiledSamplePlayback),
}

/// Apply one message on the audio thread. Bounded and allocation-free;
/// heap values that must be released are sent back through `garbage`.
pub fn apply_audio_message(
    engine: &mut MultiTrackEngine,
    message: AudioMessage,
    garbage: &Sender<Garbage>,
) {
    let release = |item: Garbage| {
        if let Err(error) = garbage.try_send(item) {
            // Never free on the audio thread; leaking is the bounded fallback
            // when the control side has stopped draining.
            std::mem::forget(error.into_inner());
        }
    };
    match message {
        AudioMessage::Revision {
            track,
            pattern,
            change,
        } => {
            let _ = engine.queue_library_revision(track, pattern, change);
        }
        AudioMessage::SynthPatch { track, patch } => {
            engine.apply_synth_patch(track, patch);
        }
        AudioMessage::Track(command) => {
            let _ = engine.apply_command(command);
        }
        AudioMessage::MasterGain(gain) => {
            let _ = engine.set_master_gain(gain);
        }
        AudioMessage::EffectParam {
            location,
            slot,
            param,
            value,
        } => {
            let _ = engine.set_effect_param(location, slot, param, value);
        }
        AudioMessage::EffectBypass {
            location,
            slot,
            bypassed,
        } => {
            let _ = engine.set_effect_bypass(location, slot, bypassed);
        }
        AudioMessage::LaunchScene { scene, frame } => {
            let _ = engine.launch_scene(scene, frame);
        }
        AudioMessage::StartChain { chain, frame } => {
            let _ = engine.start_chain(chain, frame);
        }
        AudioMessage::StopChain => engine.stop_chain(),
        AudioMessage::PauseChain(paused) => engine.pause_chain(paused),
        AudioMessage::TogglePlay => {
            engine.toggle_playing_all();
        }
        AudioMessage::Restart => engine.restart_all(),
        AudioMessage::Panic => engine.panic_all(),
        AudioMessage::AddTrack(track) => {
            if let Err((_, track)) = engine.add_track(track) {
                release(Garbage::Track(track));
            }
        }
        AudioMessage::ReplaceSample { track, playback } => {
            match engine.replace_sample(track, playback) {
                Ok(old) => release(Garbage::Playback(old)),
                Err((_, rejected)) => release(Garbage::Playback(rejected)),
            }
        }
    }
}

const NONE_U32: u32 = u32::MAX;
const NONE_U64: u64 = u64::MAX;

#[derive(Debug)]
struct TrackTelemetry {
    id: AtomicU32,
    peak: AtomicU32,
    muted: AtomicBool,
    soloed: AtomicBool,
    audible: AtomicBool,
    gain: AtomicU32,
    pan: AtomicU32,
    active_pattern: AtomicU32,
    active_revision: AtomicU64,
    queued_revision: AtomicU64,
    lock_count: AtomicU32,
}

impl Default for TrackTelemetry {
    fn default() -> Self {
        Self {
            id: AtomicU32::new(NONE_U32),
            peak: AtomicU32::new(0),
            muted: AtomicBool::new(false),
            soloed: AtomicBool::new(false),
            audible: AtomicBool::new(true),
            gain: AtomicU32::new(1.0_f32.to_bits()),
            pan: AtomicU32::new(0),
            active_pattern: AtomicU32::new(0),
            active_revision: AtomicU64::new(0),
            queued_revision: AtomicU64::new(NONE_U64),
            lock_count: AtomicU32::new(0),
        }
    }
}

/// Latest-value telemetry published by the audio thread with relaxed
/// atomics. Readers may see a mix of two publications; every value is
/// individually valid and meters are expendable by design.
#[derive(Debug)]
pub struct EngineTelemetry {
    position: AtomicU64,
    playing: AtomicBool,
    active_scene: AtomicU32,
    queued_scene: AtomicU32,
    queued_frame: AtomicU64,
    chain: AtomicU64,
    chain_next: AtomicU64,
    master_peak: AtomicU32,
    track_count: AtomicU32,
    tracks: [TrackTelemetry; MAX_REALTIME_TRACKS],
}

impl Default for EngineTelemetry {
    fn default() -> Self {
        Self {
            position: AtomicU64::new(0),
            playing: AtomicBool::new(true),
            active_scene: AtomicU32::new(NONE_U32),
            queued_scene: AtomicU32::new(NONE_U32),
            queued_frame: AtomicU64::new(0),
            chain: AtomicU64::new(NONE_U64),
            chain_next: AtomicU64::new(0),
            master_peak: AtomicU32::new(0),
            track_count: AtomicU32::new(0),
            tracks: std::array::from_fn(|_| TrackTelemetry::default()),
        }
    }
}

/// Plain copy of one telemetry publication.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TelemetrySnapshot {
    pub position_frame: u64,
    pub playing: bool,
    pub active_scene: Option<SceneId>,
    pub queued_scene: Option<(SceneId, u64)>,
    pub chain: Option<(ChainId, usize, bool)>,
    pub chain_next_frame: u64,
    pub master_peak: f32,
    pub tracks: Vec<TrackTelemetrySnapshot>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackTelemetrySnapshot {
    pub id: TrackId,
    pub peak: f32,
    pub muted: bool,
    pub soloed: bool,
    pub audible: bool,
    pub gain: f32,
    pub pan: f32,
    pub active_pattern: PatternId,
    pub active_revision: u64,
    pub queued_revision: Option<u64>,
    pub lock_count: u32,
}

impl EngineTelemetry {
    /// Publish engine state. Allocation-free; call from the audio thread at
    /// a bounded cadence (for example once per callback).
    pub fn publish(&self, engine: &MultiTrackEngine) {
        self.position
            .store(engine.position_frame(), Ordering::Relaxed);
        self.playing.store(engine.is_playing(), Ordering::Relaxed);
        self.active_scene.store(
            engine.active_scene().map_or(NONE_U32, |id| u32::from(id.0)),
            Ordering::Relaxed,
        );
        match engine.queued_scene() {
            Some((id, frame)) => {
                self.queued_scene.store(u32::from(id.0), Ordering::Relaxed);
                self.queued_frame.store(frame, Ordering::Relaxed);
            }
            None => self.queued_scene.store(NONE_U32, Ordering::Relaxed),
        }
        match engine.chain_status() {
            Some(ChainStatus {
                chain,
                step,
                next_change_frame,
                paused,
                ..
            }) => {
                let packed = (u64::from(chain.0) << 32)
                    | ((step as u64 & 0xFFFF) << 16)
                    | u64::from(paused);
                self.chain.store(packed, Ordering::Relaxed);
                self.chain_next.store(next_change_frame, Ordering::Relaxed);
            }
            None => self.chain.store(NONE_U64, Ordering::Relaxed),
        }
        self.master_peak
            .store(engine.master_peak().to_bits(), Ordering::Relaxed);
        let count = engine.track_count().min(MAX_REALTIME_TRACKS);
        for (index, slot) in self.tracks.iter().enumerate().take(count) {
            let Some(status) = engine.track_status(index) else {
                continue;
            };
            slot.id.store(u32::from(status.id.0), Ordering::Relaxed);
            slot.peak.store(status.peak.to_bits(), Ordering::Relaxed);
            slot.muted.store(status.muted, Ordering::Relaxed);
            slot.soloed.store(status.soloed, Ordering::Relaxed);
            slot.audible.store(status.audible, Ordering::Relaxed);
            slot.gain.store(status.gain.to_bits(), Ordering::Relaxed);
            slot.pan.store(status.pan.to_bits(), Ordering::Relaxed);
            slot.active_pattern
                .store(u32::from(status.active_pattern.0), Ordering::Relaxed);
            slot.active_revision
                .store(status.active_revision, Ordering::Relaxed);
            slot.queued_revision
                .store(status.queued_revision.unwrap_or(NONE_U64), Ordering::Relaxed);
            slot.lock_count
                .store(status.lock_count as u32, Ordering::Relaxed);
        }
        self.track_count.store(count as u32, Ordering::Relaxed);
    }

    pub fn position_frame(&self) -> u64 {
        self.position.load(Ordering::Relaxed)
    }

    pub fn snapshot(&self) -> TelemetrySnapshot {
        let scene = |value: u32| (value != NONE_U32).then_some(SceneId(value as u16));
        let chain = self.chain.load(Ordering::Relaxed);
        let count = (self.track_count.load(Ordering::Relaxed) as usize).min(MAX_REALTIME_TRACKS);
        TelemetrySnapshot {
            position_frame: self.position.load(Ordering::Relaxed),
            playing: self.playing.load(Ordering::Relaxed),
            active_scene: scene(self.active_scene.load(Ordering::Relaxed)),
            queued_scene: scene(self.queued_scene.load(Ordering::Relaxed))
                .map(|id| (id, self.queued_frame.load(Ordering::Relaxed))),
            chain: (chain != NONE_U64).then(|| {
                (
                    ChainId((chain >> 32) as u16),
                    ((chain >> 16) & 0xFFFF) as usize,
                    chain & 1 == 1,
                )
            }),
            chain_next_frame: self.chain_next.load(Ordering::Relaxed),
            master_peak: f32::from_bits(self.master_peak.load(Ordering::Relaxed)),
            tracks: self.tracks[..count]
                .iter()
                .map(|slot| TrackTelemetrySnapshot {
                    id: TrackId(slot.id.load(Ordering::Relaxed) as u16),
                    peak: f32::from_bits(slot.peak.load(Ordering::Relaxed)),
                    muted: slot.muted.load(Ordering::Relaxed),
                    soloed: slot.soloed.load(Ordering::Relaxed),
                    audible: slot.audible.load(Ordering::Relaxed),
                    gain: f32::from_bits(slot.gain.load(Ordering::Relaxed)),
                    pan: f32::from_bits(slot.pan.load(Ordering::Relaxed)),
                    active_pattern: PatternId(slot.active_pattern.load(Ordering::Relaxed) as u16),
                    active_revision: slot.active_revision.load(Ordering::Relaxed),
                    queued_revision: Some(slot.queued_revision.load(Ordering::Relaxed))
                        .filter(|value| *value != NONE_U64),
                    lock_count: slot.lock_count.load(Ordering::Relaxed),
                })
                .collect(),
        }
    }
}

/// What a MIDI message did after mapping.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MidiDisposition {
    /// True when learn or a mapping consumed the message, so it must not
    /// also play the performance synth.
    pub consumed: bool,
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct MixerMirror {
    gain: f32,
    pan: f32,
    muted: bool,
    soloed: bool,
    sends: [f32; 2],
}

/// Control-thread model of a running multitrack session.
pub struct SessionController {
    project: MultiTrackProject,
    project_path: Option<PathBuf>,
    sample_rate: u32,
    editors: ProjectPatternEditors,
    synth_patches: Vec<(TrackId, SynthPatch)>,
    mixer: Vec<(TrackId, MixerMirror)>,
    master_gain: f32,
    learn: MidiLearn,
    variations: Vec<(TrackId, VariationSession)>,
    selected_step: usize,
    /// Last launched or observed scene, for next/prev.
    last_scene: Option<SceneId>,
    sender: Sender<AudioMessage>,
    dropped_messages: u64,
    dirty: bool,
}

impl SessionController {
    pub fn new(
        project: MultiTrackProject,
        project_path: Option<PathBuf>,
        sample_rate: u32,
        sender: Sender<AudioMessage>,
    ) -> Result<Self, String> {
        project.validate()?;
        let mut editors = ProjectPatternEditors::new(
            project
                .tracks
                .iter()
                .map(|track| (track.id, track.pattern.clone())),
            EDIT_HISTORY_CAPACITY,
        )?;
        for track in &project.tracks {
            for entry in &track.pattern_library {
                editors.add_pattern(track.id, entry.id, entry.pattern.clone())?;
            }
        }
        let synth_patches = project
            .tracks
            .iter()
            .filter(|track| track.kind == TrackKind::Synth)
            .map(|track| {
                (
                    track.id,
                    track
                        .synth_patch
                        .unwrap_or_else(|| SynthPatch::legacy(crate::Oscillator::Saw)),
                )
            })
            .collect();
        let mixer = project
            .tracks
            .iter()
            .map(|track| {
                (
                    track.id,
                    MixerMirror {
                        gain: track.gain,
                        pan: track.pan,
                        muted: track.muted,
                        soloed: track.soloed,
                        sends: [track.send_a, track.send_b],
                    },
                )
            })
            .collect();
        let mut learn = MidiLearn::new(project.midi_mappings.clone())?;
        let orphans = {
            let exists = |target: ParameterTarget| target_exists(&project, target);
            learn.disable_orphans(exists)
        };
        let variations = project
            .tracks
            .iter()
            .map(|track| (track.id, VariationSession::new()))
            .collect();
        let master_gain = project.master_gain;
        let mut controller = Self {
            project,
            project_path,
            sample_rate,
            editors,
            synth_patches,
            mixer,
            master_gain,
            learn,
            variations,
            selected_step: 0,
            last_scene: None,
            sender,
            dropped_messages: 0,
            dirty: false,
        };
        if !orphans.is_empty() {
            controller.dirty = true;
        }
        Ok(controller)
    }

    pub fn project(&self) -> &MultiTrackProject {
        &self.project
    }

    pub fn editors(&self) -> &ProjectPatternEditors {
        &self.editors
    }

    pub fn selected_track(&self) -> TrackId {
        self.editors.selected_track()
    }

    pub fn selected_step(&self) -> usize {
        self.selected_step
    }

    pub fn set_selected_step(&mut self, step: usize) {
        let len = self
            .editors
            .editor(self.selected_track())
            .map_or(1, |editor| editor.pattern().len());
        self.selected_step = step.min(len.saturating_sub(1));
    }

    pub fn select_track(&mut self, track: TrackId) -> Result<(), String> {
        self.editors
            .apply(crate::PatternEditCommand::SelectTrack(track))?;
        self.set_selected_step(self.selected_step);
        Ok(())
    }

    /// Follow the engine's active scene (chains change it on their own).
    pub fn observe_active_scene(&mut self, scene: Option<SceneId>) {
        if scene.is_some() {
            self.last_scene = scene;
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn dropped_messages(&self) -> u64 {
        self.dropped_messages
    }

    pub fn learn(&self) -> &MidiLearn {
        &self.learn
    }

    pub fn master_gain(&self) -> f32 {
        self.master_gain
    }

    fn bpm(&self) -> f64 {
        self.project.bpm
    }

    fn steps_per_beat(&self) -> u32 {
        u32::from(self.project.steps_per_beat)
    }

    fn send(&mut self, message: AudioMessage) -> Result<(), String> {
        match self.sender.try_send(message) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.dropped_messages += 1;
                Err("audio control queue is full; change not applied".into())
            }
            Err(TrySendError::Disconnected(_)) => Err("audio engine is not running".into()),
        }
    }

    fn boundary_frame(&self, current_frame: u64, boundary: QuantizeBoundary) -> Result<u64, String> {
        crate::next_boundary_frame(
            current_frame,
            self.sample_rate,
            self.bpm(),
            self.steps_per_beat(),
            boundary,
        )
    }

    fn track_kind(&self, track: TrackId) -> Option<TrackKind> {
        self.project
            .tracks
            .iter()
            .find(|definition| definition.id == track)
            .map(|definition| definition.kind)
    }

    fn mixer_mut(&mut self, track: TrackId) -> Result<&mut MixerMirror, String> {
        self.mixer
            .iter_mut()
            .find(|(id, _)| *id == track)
            .map(|(_, mirror)| mirror)
            .ok_or_else(|| format!("track id {} does not exist", track.0))
    }

    /// Queue the selected pattern's current editor state at `boundary`.
    fn queue_selected(&mut self, current_frame: u64, boundary: QuantizeBoundary) -> Result<String, String> {
        let track = self.editors.selected_track();
        let pattern = self.editors.selected_pattern();
        let (_, change) = self.editors.queue_selected_revision(
            current_frame,
            self.sample_rate,
            self.bpm(),
            self.steps_per_beat(),
            boundary,
        )?;
        let (revision, frame) = (change.value.revision, change.apply_at_frame);
        self.send(AudioMessage::Revision {
            track,
            pattern,
            change,
        })?;
        self.sync_pattern_to_project(track, pattern);
        Ok(format!(
            "queued track {} pattern {} revision {revision} for frame {frame}",
            track.0, pattern.0
        ))
    }

    fn sync_pattern_to_project(&mut self, track: TrackId, pattern: PatternId) {
        let Some(edited) = self
            .editors
            .editor_for(track, pattern)
            .map(|editor| editor.pattern().clone())
        else {
            return;
        };
        if let Some(definition) = self.project.tracks.iter_mut().find(|t| t.id == track) {
            if pattern == crate::PRIMARY_PATTERN_ID {
                definition.pattern = edited;
            } else if let Some(entry) = definition
                .pattern_library
                .iter_mut()
                .find(|entry| entry.id == pattern)
            {
                entry.pattern = edited;
            }
        }
        self.dirty = true;
    }

    /// Execute one command line. Returns a status message.
    pub fn execute(&mut self, line: &str, current_frame: u64) -> Result<String, String> {
        let line = line.trim();
        let mut words = line.split_whitespace();
        let Some(head) = words.next() else {
            return Ok(String::new());
        };
        match head {
            "track" | "step" | "length" | "swing" | "rotate" | "undo" | "redo" => {
                self.pattern_edit(line, current_frame)
            }
            "pattern" => self.pattern_command(&line[head.len()..]),
            "patterns" => Ok(self.describe_patterns()),
            "synth" => self.synth_command(line),
            "gain" | "pan" | "mute" | "solo" | "send" | "master" => self.mixer_command(line),
            "fx" => self.fx_command(line),
            "scene" | "scenes" => self.scene_command(line, current_frame),
            "chain" => self.chain_command(line, current_frame),
            "lock" | "unlock" | "locks" => self.lock_command(line, current_frame),
            "learn" | "unlearn" | "mappings" | "mapping" => self.learn_command(line, current_frame),
            "variation" => self.variation_command(line, current_frame),
            "play" | "pause" => {
                self.send(AudioMessage::TogglePlay)?;
                Ok("transport toggled".into())
            }
            "restart" => {
                self.send(AudioMessage::Restart)?;
                Ok("restarted from bar 1".into())
            }
            "panic" => {
                self.send(AudioMessage::Panic)?;
                Ok("all voices stopped".into())
            }
            "save" => {
                let path = line[head.len()..].trim();
                self.save((!path.is_empty()).then(|| PathBuf::from(path)))
            }
            "help" => Ok(HELP_TEXT.into()),
            other => Err(format!("unknown command: {other} (type help)")),
        }
    }

    fn pattern_edit(&mut self, line: &str, current_frame: u64) -> Result<String, String> {
        let command = parse_pattern_edit_command(line)?;
        if let crate::PatternEditCommand::Step { index, .. } = command {
            self.selected_step = index;
        }
        let boundary = self.editors.default_quantize_boundary(&command);
        let outcome = self.editors.apply(command)?;
        if !outcome.changed {
            self.set_selected_step(self.selected_step);
            return Ok(format!("selected track {}", outcome.track.0));
        }
        self.queue_selected(current_frame, boundary)
    }

    fn pattern_command(&mut self, rest: &str) -> Result<String, String> {
        let id: u16 = rest
            .trim()
            .parse()
            .map_err(|_| "expected pattern <id>".to_string())?;
        self.editors.select_pattern(PatternId(id))?;
        self.set_selected_step(self.selected_step);
        Ok(format!(
            "editing track {} pattern {id}",
            self.selected_track().0
        ))
    }

    fn describe_patterns(&self) -> String {
        let track = self.selected_track();
        let ids: Vec<String> = self
            .editors
            .pattern_ids(track)
            .into_iter()
            .map(|id| {
                let name = self
                    .editors
                    .editor_for(track, id)
                    .map(|editor| editor.pattern().name.clone())
                    .unwrap_or_default();
                let marker = if id == self.editors.selected_pattern() {
                    "*"
                } else {
                    ""
                };
                format!("{marker}{}:{name}", id.0)
            })
            .collect();
        format!("track {} patterns: {}", track.0, ids.join(" "))
    }

    fn synth_patch_mut(&mut self, track: TrackId) -> Result<&mut SynthPatch, String> {
        self.synth_patches
            .iter_mut()
            .find(|(id, _)| *id == track)
            .map(|(_, patch)| patch)
            .ok_or_else(|| "selected track is not a synth track".to_string())
    }

    fn apply_synth_value(
        &mut self,
        track: TrackId,
        id: SynthParamId,
        value: SynthParamValue,
    ) -> Result<String, String> {
        let sample_rate = self.sample_rate as f32;
        let current = *self.synth_patch_mut(track)?;
        let patch = current.with_parameter(id, value, sample_rate)?;
        let compiled = CompiledSynthPatch::new(sample_rate, patch)?;
        self.send(AudioMessage::SynthPatch {
            track,
            patch: compiled,
        })?;
        // The control snapshot only changes once the audio queue accepted it.
        *self.synth_patch_mut(track)? = patch;
        if let Some(definition) = self.project.tracks.iter_mut().find(|t| t.id == track) {
            definition.synth_patch = Some(patch);
        }
        self.dirty = true;
        Ok(format!("queued synth parameter {id:?} for track {}", track.0))
    }

    fn synth_command(&mut self, line: &str) -> Result<String, String> {
        let (id, value) = parse_synth_parameter_command(line)?;
        self.apply_synth_value(self.selected_track(), id, value)
    }

    fn mixer_command(&mut self, line: &str) -> Result<String, String> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let track = self.selected_track();
        let parse_value = |index: usize| -> Result<f32, String> {
            parts
                .get(index)
                .ok_or_else(|| "missing value".to_string())?
                .parse::<f32>()
                .ok()
                .filter(|value| value.is_finite())
                .ok_or_else(|| "value must be a finite number".to_string())
        };
        let toggle = |current: bool, word: Option<&&str>| match word.copied() {
            Some("on") => Ok(true),
            Some("off") => Ok(false),
            None => Ok(!current),
            Some(other) => Err(format!("expected on or off, got {other}")),
        };
        match parts[0] {
            "gain" => self.set_parameter(
                ParameterTarget::Track {
                    track,
                    param: TrackParamId::Gain,
                },
                parse_value(1)?,
            ),
            "pan" => self.set_parameter(
                ParameterTarget::Track {
                    track,
                    param: TrackParamId::Pan,
                },
                parse_value(1)?,
            ),
            "mute" => {
                let muted = toggle(self.mixer_mut(track)?.muted, parts.get(1))?;
                self.set_parameter(
                    ParameterTarget::Track {
                        track,
                        param: TrackParamId::Mute,
                    },
                    f32::from(u8::from(muted)),
                )
            }
            "solo" => {
                let soloed = toggle(self.mixer_mut(track)?.soloed, parts.get(1))?;
                self.set_parameter(
                    ParameterTarget::Track {
                        track,
                        param: TrackParamId::Solo,
                    },
                    f32::from(u8::from(soloed)),
                )
            }
            "send" => {
                let param = match parts.get(1).copied() {
                    Some("a") | Some("A") => TrackParamId::SendA,
                    Some("b") | Some("B") => TrackParamId::SendB,
                    _ => return Err("expected send a|b <level>".into()),
                };
                self.set_parameter(ParameterTarget::Track { track, param }, parse_value(2)?)
            }
            "master" => {
                let value = if parts.get(1) == Some(&"gain") {
                    parse_value(2)?
                } else {
                    parse_value(1)?
                };
                self.set_parameter(ParameterTarget::Global(GlobalParamId::MasterGain), value)
            }
            _ => Err("unknown mixer command".into()),
        }
    }

    /// Set any addressable parameter's base value (used by commands, MIDI
    /// learn and the inspector).
    pub fn set_parameter(&mut self, target: ParameterTarget, value: f32) -> Result<String, String> {
        let descriptor = self
            .descriptor(target)
            .ok_or_else(|| format!("{} does not exist", format_target(target)))?;
        if !value.is_finite() || !(descriptor.min..=descriptor.max).contains(&value) {
            return Err(format!(
                "{} must be between {} and {}",
                format_target(target),
                descriptor.min,
                descriptor.max
            ));
        }
        match target {
            ParameterTarget::Global(GlobalParamId::MasterGain) => {
                self.send(AudioMessage::MasterGain(value))?;
                self.master_gain = value;
                self.project.master_gain = value;
            }
            ParameterTarget::Track { track, param } => {
                let command = match param {
                    TrackParamId::Gain => TrackCommand::SetGain { track, gain: value },
                    TrackParamId::Pan => TrackCommand::SetPan { track, pan: value },
                    TrackParamId::SendA => TrackCommand::SetSend {
                        track,
                        bus: 0,
                        level: value,
                    },
                    TrackParamId::SendB => TrackCommand::SetSend {
                        track,
                        bus: 1,
                        level: value,
                    },
                    TrackParamId::Mute => TrackCommand::SetMute {
                        track,
                        muted: value >= 0.5,
                    },
                    TrackParamId::Solo => TrackCommand::SetSolo {
                        track,
                        soloed: value >= 0.5,
                    },
                };
                self.send(AudioMessage::Track(command))?;
                let mirror = self.mixer_mut(track)?;
                match param {
                    TrackParamId::Gain => mirror.gain = value,
                    TrackParamId::Pan => mirror.pan = value,
                    TrackParamId::SendA => mirror.sends[0] = value,
                    TrackParamId::SendB => mirror.sends[1] = value,
                    TrackParamId::Mute => mirror.muted = value >= 0.5,
                    TrackParamId::Solo => mirror.soloed = value >= 0.5,
                }
                let mirror = *mirror;
                if let Some(definition) = self.project.tracks.iter_mut().find(|t| t.id == track) {
                    definition.gain = mirror.gain;
                    definition.pan = mirror.pan;
                    definition.muted = mirror.muted;
                    definition.soloed = mirror.soloed;
                    definition.send_a = mirror.sends[0];
                    definition.send_b = mirror.sends[1];
                }
            }
            ParameterTarget::Synth { track, param } => {
                let value = if descriptor.curve == crate::ParamCurve::Discrete {
                    value.round()
                } else {
                    value
                };
                let synth_value = match param {
                    SynthParamId::Oscillator | SynthParamId::FilterMode => {
                        return Err("oscillator and filter mode are set with synth commands".into())
                    }
                    _ => SynthParamValue::Number(value),
                };
                return self.apply_synth_value(track, param, synth_value);
            }
            ParameterTarget::Sample { .. } => {
                return Err("sample parameters are per-step locks only".into());
            }
            ParameterTarget::Effect {
                location,
                slot,
                param,
            } => {
                self.send(AudioMessage::EffectParam {
                    location,
                    slot,
                    param,
                    value,
                })?;
                if let Some(config) = effect_config_mut(&mut self.project, location, slot) {
                    let _ = config.set_param_value(param, value);
                }
            }
            ParameterTarget::Action(action) => return self.action(action, 0),
        }
        self.learn.invalidate_pickup(target);
        self.dirty = true;
        Ok(format!("{} = {value}", format_target(target)))
    }

    /// Current base value of a parameter.
    pub fn parameter_value(&self, target: ParameterTarget) -> Option<f32> {
        match target {
            ParameterTarget::Global(GlobalParamId::MasterGain) => Some(self.master_gain),
            ParameterTarget::Track { track, param } => {
                let mirror = self.mixer.iter().find(|(id, _)| *id == track)?.1;
                Some(match param {
                    TrackParamId::Gain => mirror.gain,
                    TrackParamId::Pan => mirror.pan,
                    TrackParamId::SendA => mirror.sends[0],
                    TrackParamId::SendB => mirror.sends[1],
                    TrackParamId::Mute => f32::from(u8::from(mirror.muted)),
                    TrackParamId::Solo => f32::from(u8::from(mirror.soloed)),
                })
            }
            ParameterTarget::Synth { track, param } => {
                let patch = self.synth_patches.iter().find(|(id, _)| *id == track)?.1;
                Some(synth_value(&patch, param))
            }
            ParameterTarget::Sample { param, .. } => Some(sample_param_descriptor(param).default),
            ParameterTarget::Effect {
                location,
                slot,
                param,
            } => effect_config(&self.project, location, slot).map(|config| config.param_value(param)),
            ParameterTarget::Action(_) => Some(0.0),
        }
    }

    pub fn descriptor(&self, target: ParameterTarget) -> Option<ParamDescriptor> {
        if !target_exists(&self.project, target) {
            return None;
        }
        Some(match target {
            ParameterTarget::Global(GlobalParamId::MasterGain) => MASTER_GAIN,
            ParameterTarget::Track { param, .. } => track_param_descriptor(param),
            ParameterTarget::Synth { param, .. } => synth_param_descriptor(param),
            ParameterTarget::Sample { param, .. } => sample_param_descriptor(param),
            ParameterTarget::Effect {
                location,
                slot,
                param,
            } => {
                let kind = effect_config(&self.project, location, slot)?.kind;
                *effect_params(kind).get(usize::from(param.0))?
            }
            ParameterTarget::Action(_) => ParamDescriptor {
                name: "action",
                min: 0.0,
                max: 1.0,
                default: 0.0,
                curve: crate::ParamCurve::Discrete,
                lockable: false,
            },
        })
    }

    fn action(&mut self, action: ActionId, current_frame: u64) -> Result<String, String> {
        match action {
            ActionId::TogglePlay => {
                self.send(AudioMessage::TogglePlay)?;
                Ok("transport toggled".into())
            }
            ActionId::Restart => {
                self.send(AudioMessage::Restart)?;
                Ok("restarted".into())
            }
            ActionId::Panic => {
                self.send(AudioMessage::Panic)?;
                Ok("panic".into())
            }
            ActionId::SceneNext | ActionId::ScenePrev => {
                self.step_scene(action == ActionId::SceneNext, None, current_frame)
            }
            ActionId::SceneLaunch(id) => self.launch_scene(SceneId(id), None, current_frame),
        }
    }

    fn fx_command(&mut self, line: &str) -> Result<String, String> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.get(1) == Some(&"list") || parts.len() == 1 {
            return Ok(self.describe_effects());
        }
        let location = match parts[1] {
            "track" => EffectLocation::Track(self.selected_track()),
            "send_a" | "a" => EffectLocation::Send(0),
            "send_b" | "b" => EffectLocation::Send(1),
            "master" => EffectLocation::Master,
            other => return Err(format!("unknown effect location {other}")),
        };
        let slot = EffectSlotId(
            parts
                .get(2)
                .and_then(|value| value.parse().ok())
                .ok_or("expected fx <track|send_a|send_b|master> <slot> <param> <value>")?,
        );
        let kind = effect_config(&self.project, location, slot)
            .ok_or("no effect in that slot")?
            .kind;
        let name = parts.get(3).ok_or("missing effect parameter")?;
        if *name == "bypass" {
            let bypassed = match parts.get(4).copied() {
                Some("on") => true,
                Some("off") => false,
                _ => return Err("expected bypass on|off".into()),
            };
            self.send(AudioMessage::EffectBypass {
                location,
                slot,
                bypassed,
            })?;
            if let Some(config) = effect_config_mut(&mut self.project, location, slot) {
                config.bypassed = bypassed;
            }
            self.dirty = true;
            return Ok(format!("{} slot {} bypass {bypassed}", kind.name(), slot.0));
        }
        let param = find_effect_param(kind, name)
            .ok_or_else(|| format!("{} has no parameter {name}", kind.name()))?;
        let value = parts
            .get(4)
            .and_then(|value| value.parse::<f32>().ok())
            .ok_or("missing numeric value")?;
        self.set_parameter(
            ParameterTarget::Effect {
                location,
                slot,
                param,
            },
            value,
        )
    }

    fn describe_effects(&self) -> String {
        let mut lines = Vec::new();
        let describe = |label: String, configs: &[crate::EffectConfig], lines: &mut Vec<String>| {
            for (slot, config) in configs.iter().enumerate() {
                let params: Vec<String> = effect_params(config.kind)
                    .iter()
                    .enumerate()
                    .map(|(index, descriptor)| {
                        format!(
                            "{}={}",
                            descriptor.name,
                            config.param_value(EffectParamId(index as u8))
                        )
                    })
                    .collect();
                lines.push(format!(
                    "{label} {slot}: {}{} {}",
                    config.kind.name(),
                    if config.bypassed { " (bypassed)" } else { "" },
                    params.join(" ")
                ));
            }
        };
        if let Some(track) = self
            .project
            .tracks
            .iter()
            .find(|track| track.id == self.selected_track())
        {
            describe(format!("track {}", track.id.0), &track.inserts, &mut lines);
        }
        for (index, bus) in self.project.effects.send_buses.iter().enumerate() {
            let label = if index == 0 { "send_a" } else { "send_b" };
            describe(label.into(), &bus.effects, &mut lines);
        }
        describe("master".into(), &self.project.effects.master, &mut lines);
        if lines.is_empty() {
            "no effects configured".into()
        } else {
            lines.join(" | ")
        }
    }

    fn scene_command(&mut self, line: &str, current_frame: u64) -> Result<String, String> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let boundary = |word: Option<&&str>| -> Result<Option<QuantizeBoundary>, String> {
            word.map(|word| crate::resample::parse_boundary(word))
                .transpose()
        };
        match (parts[0], parts.get(1).copied()) {
            ("scenes", _) | ("scene", None) | ("scene", Some("list")) => Ok(self.describe_scenes()),
            ("scene", Some("launch")) => {
                let spec = parts.get(2).ok_or("expected scene launch <name|id> [boundary]")?;
                let id = resolve_scene(&self.project.scenes, spec)?;
                self.launch_scene(id, boundary(parts.get(3))?, current_frame)
            }
            ("scene", Some("next")) => self.step_scene(true, boundary(parts.get(2))?, current_frame),
            ("scene", Some("prev")) => {
                self.step_scene(false, boundary(parts.get(2))?, current_frame)
            }
            ("scene", Some(spec)) => {
                let id = resolve_scene(&self.project.scenes, spec)?;
                self.launch_scene(id, boundary(parts.get(2))?, current_frame)
            }
            _ => Err("expected scene launch|next|prev|list".into()),
        }
    }

    fn describe_scenes(&self) -> String {
        if self.project.scenes.is_empty() {
            return "project has no scenes".into();
        }
        self.project
            .scenes
            .iter()
            .map(|scene| format!("{}:{}", scene.id.0, scene.name))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Scene index currently considered active for next/prev, taken from
    /// the caller-provided telemetry (or the first scene).
    fn step_scene(
        &mut self,
        forward: bool,
        boundary: Option<QuantizeBoundary>,
        current_frame: u64,
    ) -> Result<String, String> {
        let scenes = &self.project.scenes;
        if scenes.is_empty() {
            return Err("project has no scenes".into());
        }
        let current = self
            .last_scene
            .and_then(|id| scenes.iter().position(|scene| scene.id == id));
        let next = match (current, forward) {
            (None, _) => 0,
            (Some(index), true) => (index + 1) % scenes.len(),
            (Some(index), false) => (index + scenes.len() - 1) % scenes.len(),
        };
        let id = scenes[next].id;
        self.launch_scene(id, boundary, current_frame)
    }

    pub fn launch_scene(
        &mut self,
        id: SceneId,
        boundary: Option<QuantizeBoundary>,
        current_frame: u64,
    ) -> Result<String, String> {
        let scene = self
            .project
            .scenes
            .iter()
            .find(|scene| scene.id == id)
            .ok_or_else(|| format!("unknown scene {}", id.0))?;
        let name = scene.name.clone();
        let frame = self.boundary_frame(
            current_frame,
            boundary.unwrap_or(QuantizeBoundary::Bar {
                beats_per_bar: BEATS_PER_BAR,
            }),
        )?;
        self.send(AudioMessage::LaunchScene { scene: id, frame })?;
        self.last_scene = Some(id);
        Ok(format!("scene {name} queued for frame {frame}"))
    }

    fn chain_command(&mut self, line: &str, current_frame: u64) -> Result<String, String> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts.get(1).copied() {
            Some("start") => {
                let spec = parts.get(2).ok_or("expected chain start <name|id>")?;
                let id = resolve_chain(&self.project.chains, spec)?;
                self.send(AudioMessage::StartChain {
                    chain: id,
                    frame: current_frame,
                })?;
                Ok(format!("chain {spec} starts on the next bar"))
            }
            Some("stop") => {
                self.send(AudioMessage::StopChain)?;
                Ok("chain stopped".into())
            }
            Some("pause") => {
                self.send(AudioMessage::PauseChain(true))?;
                Ok("chain paused".into())
            }
            Some("resume") => {
                self.send(AudioMessage::PauseChain(false))?;
                Ok("chain resumed".into())
            }
            _ => Ok(if self.project.chains.is_empty() {
                "project has no chains".into()
            } else {
                self.project
                    .chains
                    .iter()
                    .map(|chain| format!("{}:{} ({} steps)", chain.id.0, chain.name, chain.steps.len()))
                    .collect::<Vec<_>>()
                    .join(" ")
            }),
        }
    }

    /// Resolve a lock target typed by the user for the selected track.
    pub fn parse_lock_target(&self, spec: &str) -> Result<LockTarget, String> {
        let track = self.selected_track();
        // fx.<slot>.<param name> on this track's inserts.
        let parts: Vec<&str> = spec.split('.').collect();
        if parts.len() == 3 && parts[0] == "fx" {
            if let Ok(slot) = parts[1].parse::<u8>() {
                let kind = effect_config(&self.project, EffectLocation::Track(track), EffectSlotId(slot))
                    .ok_or("no insert effect in that slot")?
                    .kind;
                let param = parts[2]
                    .parse::<u8>()
                    .ok()
                    .map(EffectParamId)
                    .or_else(|| find_effect_param(kind, parts[2]))
                    .ok_or_else(|| format!("{} has no parameter {}", kind.name(), parts[2]))?;
                return Ok(LockTarget::Effect {
                    slot: EffectSlotId(slot),
                    param,
                });
            }
        }
        match crate::parse_target(spec, track)? {
            ParameterTarget::Track { track: owner, param } if owner == track => {
                Ok(LockTarget::Track(param))
            }
            ParameterTarget::Synth { track: owner, param } if owner == track => {
                Ok(LockTarget::Synth(param))
            }
            ParameterTarget::Sample { track: owner, param } if owner == track => {
                Ok(LockTarget::Sample(param))
            }
            ParameterTarget::Effect {
                location: EffectLocation::Track(owner),
                slot,
                param,
            } if owner == track => Ok(LockTarget::Effect { slot, param }),
            _ => Err("locks can only target parameters of the selected track".into()),
        }
    }

    fn lock_command(&mut self, line: &str, current_frame: u64) -> Result<String, String> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        let step = self.selected_step;
        let track = self.selected_track();
        if parts[0] == "locks" {
            let editor = self.editors.editor(track).ok_or("no pattern editor")?;
            let locks = editor.pattern().step_locks(step);
            if locks.is_empty() {
                return Ok(format!("step {step} has no locks"));
            }
            return Ok(format!(
                "step {step} locks: {}",
                locks
                    .iter()
                    .map(|lock| format!("{}={}", describe_lock(lock.target), lock.value))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
        let target = self.parse_lock_target(parts.get(1).ok_or("missing lock target")?)?;
        if let LockTarget::Synth(_) = target {
            if self.track_kind(track) != Some(TrackKind::Synth) {
                return Err("synth locks need a synth track".into());
            }
        }
        if let LockTarget::Sample(_) = target {
            if self.track_kind(track) != Some(TrackKind::Sample) {
                return Err("sample locks need a sample track".into());
            }
        }
        let editor = self.editors.selected_editor_mut()?;
        let mut pattern = editor.pattern().clone();
        let mut locks = pattern.step_locks(step).to_vec();
        if parts[0] == "lock" {
            let value: f32 = parts
                .get(2)
                .and_then(|value| value.parse().ok())
                .ok_or("expected lock <target> <value>")?;
            if let LockTarget::Effect { slot, param } = target {
                let kind = effect_config(&self.project, EffectLocation::Track(track), slot)
                    .ok_or("no insert effect in that slot")?
                    .kind;
                let descriptor = effect_params(kind)
                    .get(usize::from(param.0))
                    .ok_or("effect parameter does not exist")?;
                if !descriptor.lockable {
                    return Err(format!("{} is not lockable", descriptor.name));
                }
                if !(descriptor.min..=descriptor.max).contains(&value) {
                    return Err(format!(
                        "{} must be between {} and {}",
                        descriptor.name, descriptor.min, descriptor.max
                    ));
                }
            }
            target.validate_value(value)?;
            match locks.iter_mut().find(|lock| lock.target == target) {
                Some(lock) => lock.value = value,
                None => {
                    if locks.len() >= MAX_LOCKS_PER_STEP {
                        return Err(format!("steps hold at most {MAX_LOCKS_PER_STEP} locks"));
                    }
                    locks.push(ParameterLock { target, value });
                }
            }
        } else {
            let before = locks.len();
            locks.retain(|lock| lock.target != target);
            if locks.len() == before {
                return Err("that parameter is not locked on this step".into());
            }
        }
        pattern.set_step_locks(step, locks);
        let editor = self.editors.selected_editor_mut()?;
        editor.replace_pattern(pattern)?;
        self.queue_selected(current_frame, QuantizeBoundary::Step)
    }

    fn learn_command(&mut self, line: &str, now_ms: u64) -> Result<String, String> {
        let command = parse_learn_command(line, self.selected_track())?;
        let mut learn = std::mem::replace(&mut self.learn, MidiLearn::new(Vec::new())?);
        let resolver = ControllerResolver(self);
        let result = (|| match command {
            LearnCommand::Learn {
                target,
                allow_notes,
            } => {
                if resolver.descriptor(target).is_none() {
                    return Err(format!("{} does not exist", format_target(target)));
                }
                learn.begin_learn(target, now_ms, allow_notes);
                Ok(format!(
                    "learning {}: move a control (Esc or `learn cancel` to stop)",
                    format_target(target)
                ))
            }
            LearnCommand::Cancel => {
                learn.cancel_learn();
                Ok("learn cancelled".into())
            }
            LearnCommand::Confirm { policy } => {
                let conflicts = match learn.learn_state() {
                    LearnState::Captured { conflicts, .. } => conflicts.clone(),
                    _ => return Err("nothing captured to confirm".into()),
                };
                let policy = match policy {
                    Some(policy) => policy,
                    None if conflicts.is_empty() => ConflictPolicy::Replace,
                    None => {
                        return Err(format!(
                            "control already mapped ({}); use learn confirm replace|add|cancel",
                            conflicts
                                .iter()
                                .map(|id| id.0.to_string())
                                .collect::<Vec<_>>()
                                .join(",")
                        ))
                    }
                };
                match learn.confirm(policy, &resolver)? {
                    Some(id) => Ok(format!("mapping {} created", id.0)),
                    None => Ok("learn cancelled".into()),
                }
            }
            LearnCommand::Unlearn(id) => {
                if learn.remove(id) {
                    Ok(format!("mapping {} removed", id.0))
                } else {
                    Err(format!("no mapping {}", id.0))
                }
            }
            LearnCommand::List => Ok(if learn.mappings().is_empty() {
                "no MIDI mappings".into()
            } else {
                learn
                    .mappings()
                    .iter()
                    .filter_map(|mapping| learn.describe(mapping.id))
                    .collect::<Vec<_>>()
                    .join(" | ")
            }),
            LearnCommand::SetRange { id, min, max } => {
                learn.set_range(id, min, max).map(|()| "mapping updated".into())
            }
            LearnCommand::SetInverted { id, inverted } => learn
                .set_inverted(id, inverted)
                .map(|()| "mapping updated".into()),
            LearnCommand::SetPickup { id, mode } => {
                learn.set_pickup(id, mode).map(|()| "mapping updated".into())
            }
            LearnCommand::SetButton { id, mode } => {
                learn.set_button(id, mode).map(|()| "mapping updated".into())
            }
        })();
        self.learn = learn;
        self.persist_mappings();
        result
    }

    fn persist_mappings(&mut self) {
        if self.project.midi_mappings != self.learn.mappings() {
            self.project.midi_mappings = self.learn.mappings().to_vec();
            self.dirty = true;
        }
    }

    /// Route one MIDI message through learn and the mapping table.
    pub fn handle_midi(
        &mut self,
        port: &str,
        event: MidiEvent,
        now_ms: u64,
        current_frame: u64,
    ) -> MidiDisposition {
        let mut outputs = Vec::new();
        let mut learn = std::mem::replace(
            &mut self.learn,
            MidiLearn::new(Vec::new()).expect("empty mapping set is valid"),
        );
        let resolver = ControllerResolver(self);
        let matched = !learn.matching_ids(port, event).is_empty();
        let progress = learn.handle(port, event, now_ms, &resolver, &mut outputs);
        self.learn = learn;
        let mut messages = Vec::new();
        let consumed = match progress {
            LearnProgress::Captured { conflicts } => {
                messages.push(if conflicts.is_empty() {
                    "control captured; `learn confirm` to map it".into()
                } else {
                    "control captured but already mapped; `learn confirm replace|add|cancel`"
                        .into()
                });
                true
            }
            LearnProgress::Ignored => true,
            LearnProgress::NotLearning => matched,
        };
        for output in outputs {
            let result = match output {
                MappedOutput::SetParameter { target, value } => self.set_parameter(target, value),
                MappedOutput::Action(action) => self.action(action, current_frame),
            };
            match result {
                Ok(message) => messages.push(message),
                Err(error) => messages.push(format!("mapping error: {error}")),
            }
        }
        MidiDisposition { consumed, messages }
    }

    /// Expire a pending learn. Returns a message when it timed out.
    pub fn tick(&mut self, now_ms: u64) -> Option<String> {
        self.learn
            .tick(now_ms)
            .then(|| "MIDI learn timed out; mappings unchanged".to_string())
    }

    pub fn learning(&self) -> Option<String> {
        match self.learn.learn_state() {
            LearnState::Idle => None,
            LearnState::Awaiting { target, .. } => {
                Some(format!("learn {}: move a control", format_target(*target)))
            }
            LearnState::Captured { target, source, .. } => Some(format!(
                "learn {}: captured {:?}; Enter confirms",
                format_target(*target),
                source.message
            )),
        }
    }

    fn variation_command(&mut self, line: &str, current_frame: u64) -> Result<String, String> {
        let command = parse_variation_command(line)?;
        let track = self.selected_track();
        match command {
            VariationCommand::ShowLocks => {
                let editor = self.editors.editor(track).ok_or("no pattern editor")?;
                Ok(format_locks(editor.pattern().invariants.as_ref()).join(" | "))
            }
            VariationCommand::Preview { seed, amount } => {
                let project_seed = self.project.seed;
                let editor = self.editors.editor(track).ok_or("no pattern editor")?.clone();
                let session = self.variation_mut(track)?;
                let seed =
                    seed.unwrap_or_else(|| session.next_derived_seed(project_seed, editor.revision()));
                match session.preview(&editor, VariationRequest { seed, amount }) {
                    Ok(proposal) => Ok(format_proposal(proposal).join(" | ")),
                    Err(failure) => Err(failure.message),
                }
            }
            VariationCommand::Accept => {
                let revision = self
                    .editors
                    .editor(track)
                    .ok_or("no pattern editor")?
                    .revision();
                let proposal = self.variation_mut(track)?.take_for_accept(revision)?;
                // Commit only after the audio queue accepted the revision.
                let snapshot = self.editors.selected_editor_mut()?.clone();
                self.editors
                    .selected_editor_mut()?
                    .replace_pattern(proposal.candidate.clone())?;
                let steps = proposal.candidate.len() as u32;
                match self.queue_selected(current_frame, QuantizeBoundary::Pattern { steps }) {
                    Ok(message) => Ok(format!("variation accepted; {message}")),
                    Err(error) => {
                        *self.editors.selected_editor_mut()? = snapshot;
                        self.variation_mut(track)?.restore(proposal);
                        Err(error)
                    }
                }
            }
            VariationCommand::Reject => {
                if self.variation_mut(track)?.reject() {
                    Ok("variation rejected".into())
                } else {
                    Err("no variation proposal".into())
                }
            }
            lock => {
                let editor = self.editors.selected_editor_mut()?;
                let profile = apply_lock_command(editor.pattern().invariants.clone(), &lock)?;
                editor.set_invariants(profile)?;
                let pattern = self.editors.selected_pattern();
                self.sync_pattern_to_project(track, pattern);
                let editor = self.editors.editor(track).ok_or("no pattern editor")?;
                Ok(format_locks(editor.pattern().invariants.as_ref()).join(" | "))
            }
        }
    }

    fn variation_mut(&mut self, track: TrackId) -> Result<&mut VariationSession, String> {
        if !self.variations.iter().any(|(id, _)| *id == track) {
            self.variations.push((track, VariationSession::new()));
        }
        Ok(&mut self
            .variations
            .iter_mut()
            .find(|(id, _)| *id == track)
            .expect("inserted above")
            .1)
    }

    /// Save the project (current edits, mixer, mappings) atomically.
    pub fn save(&mut self, path: Option<PathBuf>) -> Result<String, String> {
        let path = path
            .or_else(|| self.project_path.clone())
            .ok_or("no project path; use save <file>")?;
        for track in self.editors.track_ids() {
            for pattern in self.editors.pattern_ids(track) {
                self.sync_pattern_to_project(track, pattern);
            }
        }
        self.project.revision = self.project.revision.saturating_add(1);
        self.project.schema_version = crate::MULTITRACK_PROJECT_SCHEMA_VERSION;
        self.project.save_atomic(&path)?;
        self.project_path = Some(path.clone());
        self.dirty = false;
        Ok(format!("project saved to {}", path.display()))
    }

    /// Register a finished resample as a sample track (new or replaced).
    pub fn add_sample_track(
        &mut self,
        definition: crate::TrackDefinition,
        prepared: PreparedTrack,
    ) -> Result<String, String> {
        if self.project.tracks.len() >= MAX_REALTIME_TRACKS {
            return Err(format!("projects hold at most {MAX_REALTIME_TRACKS} tracks"));
        }
        let id = definition.id;
        self.send(AudioMessage::AddTrack(prepared))?;
        self.editors.add_pattern(
            id,
            crate::PRIMARY_PATTERN_ID,
            definition.pattern.clone(),
        )?;
        self.mixer.push((
            id,
            MixerMirror {
                gain: definition.gain,
                pan: definition.pan,
                muted: definition.muted,
                soloed: definition.soloed,
                sends: [definition.send_a, definition.send_b],
            },
        ));
        self.project.tracks.push(definition);
        self.dirty = true;
        Ok(format!("added sample track {}", id.0))
    }

    /// Next unused track ID.
    pub fn next_track_id(&self) -> TrackId {
        TrackId(
            self.project
                .tracks
                .iter()
                .map(|track| track.id.0)
                .max()
                .unwrap_or(0)
                .saturating_add(1),
        )
    }

    pub fn replace_track_sample(
        &mut self,
        track: TrackId,
        path: String,
        playback: CompiledSamplePlayback,
    ) -> Result<String, String> {
        if self.track_kind(track) != Some(TrackKind::Sample) {
            return Err(format!("track {} is not a sample track", track.0));
        }
        self.send(AudioMessage::ReplaceSample { track, playback })?;
        if let Some(sample) = self
            .project
            .tracks
            .iter_mut()
            .find(|definition| definition.id == track)
            .and_then(|definition| definition.sample.as_mut())
        {
            sample.path = path;
        }
        self.dirty = true;
        Ok(format!("track {} now plays the resampled audio", track.0))
    }

    pub fn project_dir(&self) -> PathBuf {
        self.project_path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn toggle_selected_step(&mut self, current_frame: u64) -> Result<String, String> {
        let line = format!("step {} toggle", self.selected_step + 1);
        self.pattern_edit(&line, current_frame)
    }
}

struct ControllerResolver<'a>(&'a SessionController);

impl TargetResolver for ControllerResolver<'_> {
    fn descriptor(&self, target: ParameterTarget) -> Option<ParamDescriptor> {
        self.0.descriptor(target)
    }

    fn current_value(&self, target: ParameterTarget) -> Option<f32> {
        self.0.parameter_value(target)
    }
}

fn describe_lock(target: LockTarget) -> String {
    match target {
        LockTarget::Track(param) => format!("track.{}", track_param_descriptor(param).name),
        LockTarget::Synth(param) => synth_param_descriptor(param).name.to_string(),
        LockTarget::Sample(param) => format!("sample.{}", sample_param_descriptor(param).name),
        LockTarget::Effect { slot, param } => format!("fx.{}.{}", slot.0, param.0),
    }
}

fn synth_value(patch: &SynthPatch, param: SynthParamId) -> f32 {
    match param {
        SynthParamId::Oscillator => patch.oscillator as u8 as f32,
        SynthParamId::FilterMode => patch.filter.mode as u8 as f32,
        SynthParamId::Octave => f32::from(patch.octave),
        SynthParamId::Semitone => f32::from(patch.semitone),
        SynthParamId::FineCents => patch.fine_cents,
        SynthParamId::PulseWidth => patch.pulse_width,
        SynthParamId::AmpAttack => patch.amp_env.attack_secs,
        SynthParamId::AmpDecay => patch.amp_env.decay_secs,
        SynthParamId::AmpSustain => patch.amp_env.sustain,
        SynthParamId::AmpRelease => patch.amp_env.release_secs,
        SynthParamId::FilterCutoff => patch.filter.cutoff_hz,
        SynthParamId::FilterResonance => patch.filter.resonance,
        SynthParamId::FilterKeytrack => patch.filter.key_tracking,
        SynthParamId::FilterAttack => patch.filter_env.attack_secs,
        SynthParamId::FilterDecay => patch.filter_env.decay_secs,
        SynthParamId::FilterSustain => patch.filter_env.sustain,
        SynthParamId::FilterRelease => patch.filter_env.release_secs,
        SynthParamId::FilterEnvAmount => patch.filter_env_amount,
        SynthParamId::OutputGain => patch.output_gain,
    }
}

fn effect_config(
    project: &MultiTrackProject,
    location: EffectLocation,
    slot: EffectSlotId,
) -> Option<&crate::EffectConfig> {
    let slot = usize::from(slot.0);
    match location {
        EffectLocation::Track(track) => project
            .tracks
            .iter()
            .find(|definition| definition.id == track)?
            .inserts
            .get(slot),
        EffectLocation::Send(bus) => project
            .effects
            .send_buses
            .get(usize::from(bus))?
            .effects
            .get(slot),
        EffectLocation::Master => project.effects.master.get(slot),
    }
}

fn effect_config_mut(
    project: &mut MultiTrackProject,
    location: EffectLocation,
    slot: EffectSlotId,
) -> Option<&mut crate::EffectConfig> {
    let slot = usize::from(slot.0);
    match location {
        EffectLocation::Track(track) => project
            .tracks
            .iter_mut()
            .find(|definition| definition.id == track)?
            .inserts
            .get_mut(slot),
        EffectLocation::Send(bus) => project
            .effects
            .send_buses
            .get_mut(usize::from(bus))?
            .effects
            .get_mut(slot),
        EffectLocation::Master => project.effects.master.get_mut(slot),
    }
}

/// Whether a target resolves in the project (orphan detection never
/// retargets by index).
pub fn target_exists(project: &MultiTrackProject, target: ParameterTarget) -> bool {
    let track_kind = |id: TrackId| {
        project
            .tracks
            .iter()
            .find(|definition| definition.id == id)
            .map(|definition| definition.kind)
    };
    match target {
        ParameterTarget::Global(_) => true,
        ParameterTarget::Track { track, .. } => track_kind(track).is_some(),
        ParameterTarget::Synth { track, param } => {
            track_kind(track) == Some(TrackKind::Synth)
                && !matches!(param, SynthParamId::Oscillator | SynthParamId::FilterMode)
        }
        ParameterTarget::Sample { track, .. } => track_kind(track) == Some(TrackKind::Sample),
        ParameterTarget::Effect {
            location,
            slot,
            param,
        } => effect_config(project, location, slot)
            .is_some_and(|config| usize::from(param.0) < effect_params(config.kind).len()),
        ParameterTarget::Action(ActionId::SceneLaunch(id)) => {
            project.scenes.iter().any(|scene| scene.id.0 == id)
        }
        ParameterTarget::Action(_) => true,
    }
}

/// Kind of every effect, for UI listings.
pub fn effect_kind_label(kind: EffectKind) -> &'static str {
    kind.name()
}

/// Sample parameter IDs for inspector listings.
pub const SAMPLE_PARAMS: [SampleParamId; 3] = [
    SampleParamId::Pitch,
    SampleParamId::Gain,
    SampleParamId::Reverse,
];

/// Synth parameters listed by the inspector and accepted by `learn`.
pub fn inspector_synth_params() -> impl Iterator<Item = SynthParamId> {
    [
        "filter_cutoff",
        "filter_resonance",
        "filter_env_amount",
        "amp_attack",
        "amp_decay",
        "amp_sustain",
        "amp_release",
        "pulse_width",
        "fine_cents",
        "output_gain",
    ]
    .into_iter()
    .filter_map(parse_synth_param)
}

pub const HELP_TEXT: &str = "commands: track N | step N toggle|note|vel|gate|prob|ratchet|micro | length N | swing X | rotate left|right N | undo | redo | pattern ID | patterns | synth PARAM VALUE | gain|pan X | mute|solo [on|off] | send a|b X | master X | fx list | fx track|send_a|send_b|master SLOT PARAM VALUE|bypass on|off | scene launch NAME [bar|beat|step|now] | scene next|prev | chain start NAME | chain stop|pause|resume | lock TARGET VALUE | unlock TARGET | locks | learn TARGET | learn confirm [replace|add|cancel] | learn cancel | mappings | mapping ID range|invert|pickup|button ... | unlearn ID | variation lock ...|preview|accept|reject | resample arm master|track [ID] [bars N|seconds S] | resample stop [bar] | resample cancel|normalize on|off|destination asset|new-track|replace | blackbox status|save [PATH]|clear | play | restart | panic | save [PATH]";
