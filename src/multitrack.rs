use crate::{
    ChannelStrip, CompiledPatternRevision, EngineCommand, LiveSequencer, Oscillator, Pattern,
    QuantizedChange, RealtimeSynth, SmoothedParam,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, File},
    io::Write,
    path::Path,
};

pub const MAX_REALTIME_TRACKS: usize = 16;
pub const MULTITRACK_PROJECT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiTrackProject {
    pub schema_version: u32,
    pub revision: u64,
    pub seed: u64,
    pub bpm: f64,
    pub steps_per_beat: u8,
    pub tracks: Vec<TrackDefinition>,
}

impl MultiTrackProject {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != MULTITRACK_PROJECT_SCHEMA_VERSION {
            return Err(format!(
                "unsupported multitrack project schema {}; expected {}",
                self.schema_version, MULTITRACK_PROJECT_SCHEMA_VERSION
            ));
        }
        if !self.bpm.is_finite() || !(20.0..=400.0).contains(&self.bpm) {
            return Err("project bpm must be finite and between 20 and 400".into());
        }
        if !(1..=64).contains(&self.steps_per_beat) {
            return Err("project steps_per_beat must be between 1 and 64".into());
        }
        validate_track_definitions(&self.tracks)
    }

    pub fn to_snapshot(&self) -> Result<EngineProjectSnapshot, String> {
        self.validate()?;
        EngineProjectSnapshot::new(self.revision, self.tracks.clone())
    }

    pub fn save_atomic(&self, path: impl AsRef<Path>) -> Result<(), String> {
        self.validate()?;
        let path = path.as_ref();
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)
            .map_err(|error| format!("create multitrack project directory: {error}"))?;

        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .map(|value| format!("{value}.tmp"))
            .unwrap_or_else(|| "tmp".to_owned());
        let temp_path = path.with_extension(extension);

        let result = (|| -> Result<(), String> {
            let json = serde_json::to_vec_pretty(self)
                .map_err(|error| format!("serialize multitrack project: {error}"))?;
            let mut file = File::create(&temp_path)
                .map_err(|error| format!("create temporary multitrack project: {error}"))?;
            file.write_all(&json)
                .map_err(|error| format!("write temporary multitrack project: {error}"))?;
            file.write_all(b"\n")
                .map_err(|error| format!("finish temporary multitrack project: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("sync temporary multitrack project: {error}"))?;
            fs::rename(&temp_path, path)
                .map_err(|error| format!("replace multitrack project file: {error}"))?;
            Ok(())
        })();

        if result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        result
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let bytes =
            fs::read(path.as_ref()).map_err(|error| format!("read multitrack project: {error}"))?;
        let project: Self = serde_json::from_slice(&bytes)
            .map_err(|error| format!("parse multitrack project: {error}"))?;
        project.validate()?;
        Ok(project)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EngineProjectSnapshot {
    revision: u64,
    tracks: Vec<TrackDefinition>,
}

impl EngineProjectSnapshot {
    pub fn new(revision: u64, tracks: Vec<TrackDefinition>) -> Result<Self, String> {
        validate_track_definitions(&tracks)?;
        Ok(Self { revision, tracks })
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn tracks(&self) -> &[TrackDefinition] {
        &self.tracks
    }

    pub fn into_tracks(self) -> Vec<TrackDefinition> {
        self.tracks
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TrackCommand {
    SetGain { track: TrackId, gain: f32 },
    SetPan { track: TrackId, pan: f32 },
    SetMute { track: TrackId, muted: bool },
    SetSolo { track: TrackId, soloed: bool },
    Panic { track: Option<TrackId> },
    RestartAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TrackId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Synth,
    Sample,
    ExternalMidi,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackDefinition {
    pub id: TrackId,
    pub name: String,
    pub kind: TrackKind,
    pub gain: f32,
    pub pan: f32,
    pub muted: bool,
    pub soloed: bool,
    pub pattern: Pattern,
}

impl TrackDefinition {
    pub fn validate(&self) -> Result<(), String> {
        if self.name.trim().is_empty() {
            return Err("track name may not be empty".into());
        }
        if !self.gain.is_finite() || !(0.0..=2.0).contains(&self.gain) {
            return Err("track gain must be finite and between 0.0 and 2.0".into());
        }
        if !self.pan.is_finite() || !(-1.0..=1.0).contains(&self.pan) {
            return Err("track pan must be finite and between -1.0 and 1.0".into());
        }
        self.pattern.validate()
    }
}

#[derive(Debug, Clone)]
struct RealtimeTrack {
    id: TrackId,
    sequencer: LiveSequencer,
    synth: RealtimeSynth,
    gain: SmoothedParam,
    pan: SmoothedParam,
    muted: bool,
    soloed: bool,
    smoothing_frames: u32,
    pending_pattern: Option<QuantizedChange<CompiledPatternRevision>>,
    active_revision: u64,
}

impl RealtimeTrack {
    fn new(
        sample_rate: u32,
        bpm: f64,
        steps_per_beat: u32,
        project_seed: u64,
        polyphony: usize,
        definition: TrackDefinition,
    ) -> Result<Self, String> {
        definition.validate()?;
        if definition.kind != TrackKind::Synth {
            return Err(format!(
                "track {:?} uses {:?}, which is not implemented by the real-time engine yet",
                definition.id, definition.kind
            ));
        }

        let sequencer = LiveSequencer::new(
            sample_rate,
            bpm,
            steps_per_beat,
            project_seed,
            definition.pattern,
        )?;
        let synth = RealtimeSynth::new(sample_rate as f32, Oscillator::Saw, polyphony)?;
        let smoothing_frames = (sample_rate / 200).max(1);

        Ok(Self {
            id: definition.id,
            sequencer,
            synth,
            gain: SmoothedParam::new(definition.gain),
            pan: SmoothedParam::new(definition.pan),
            muted: definition.muted,
            soloed: definition.soloed,
            smoothing_frames,
            pending_pattern: None,
            active_revision: 0,
        })
    }

    fn is_audible(&self, any_soloed: bool) -> bool {
        !self.muted && (!any_soloed || self.soloed)
    }

    fn next_stereo_frame(
        &mut self,
        any_soloed: bool,
        commands: &mut Vec<EngineCommand>,
    ) -> (f32, f32) {
        if let Some(change) = self.pending_pattern {
            if change.apply_at_frame <= self.sequencer.position_frame() {
                self.synth.handle(EngineCommand::Panic);
                self.sequencer
                    .replace_compiled_pattern(change.value.pattern);
                self.active_revision = change.value.revision;
                self.pending_pattern = None;
            }
        }

        self.sequencer.fill_commands(commands);
        for command in commands.drain(..) {
            self.synth.handle(command);
        }

        let mono = self.synth.next_sample();
        let gain = self.gain.next_value();
        let pan = self.pan.next_value();
        if !self.is_audible(any_soloed) {
            return (0.0, 0.0);
        }
        ChannelStrip {
            gain,
            pan,
            muted: false,
        }
        .process_mono(mono)
    }
}

#[derive(Debug, Clone)]
pub struct MultiTrackEngine {
    tracks: Vec<RealtimeTrack>,
    command_buffer: Vec<EngineCommand>,
}

impl MultiTrackEngine {
    pub fn new(
        sample_rate: u32,
        bpm: f64,
        steps_per_beat: u32,
        project_seed: u64,
        polyphony_per_track: usize,
        definitions: Vec<TrackDefinition>,
    ) -> Result<Self, String> {
        validate_track_definitions(&definitions)?;

        let mut tracks = Vec::with_capacity(definitions.len());
        for definition in definitions {
            tracks.push(RealtimeTrack::new(
                sample_rate,
                bpm,
                steps_per_beat,
                project_seed,
                polyphony_per_track,
                definition,
            )?);
        }

        Ok(Self {
            tracks,
            command_buffer: Vec::with_capacity(LiveSequencer::MAX_COMMANDS_PER_FRAME),
        })
    }

    pub fn from_snapshot(
        sample_rate: u32,
        bpm: f64,
        steps_per_beat: u32,
        project_seed: u64,
        polyphony_per_track: usize,
        snapshot: EngineProjectSnapshot,
    ) -> Result<Self, String> {
        Self::new(
            sample_rate,
            bpm,
            steps_per_beat,
            project_seed,
            polyphony_per_track,
            snapshot.into_tracks(),
        )
    }

    pub fn apply_command(&mut self, command: TrackCommand) -> Result<(), String> {
        match command {
            TrackCommand::SetGain { track, gain } => self.set_gain(track, gain),
            TrackCommand::SetPan { track, pan } => self.set_pan(track, pan),
            TrackCommand::SetMute { track, muted } => self.set_mute(track, muted),
            TrackCommand::SetSolo { track, soloed } => self.set_solo(track, soloed),
            TrackCommand::Panic { track: Some(track) } => {
                self.track_mut(track)?.synth.handle(EngineCommand::Panic);
                Ok(())
            }
            TrackCommand::Panic { track: None } => {
                self.panic_all();
                Ok(())
            }
            TrackCommand::RestartAll => {
                self.restart_all();
                Ok(())
            }
        }
    }

    pub fn next_stereo_frame(&mut self) -> (f32, f32) {
        let any_soloed = self.tracks.iter().any(|track| track.soloed);
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;

        for track in &mut self.tracks {
            let (track_left, track_right) =
                track.next_stereo_frame(any_soloed, &mut self.command_buffer);
            left += track_left;
            right += track_right;
        }

        (crate::protect_master(left), crate::protect_master(right))
    }

    pub fn track_ids(&self) -> Vec<TrackId> {
        self.tracks.iter().map(|track| track.id).collect()
    }

    pub fn position_frame(&self) -> u64 {
        self.tracks
            .first()
            .map(|track| track.sequencer.position_frame())
            .unwrap_or(0)
    }

    pub fn track_position(&self, id: TrackId) -> Option<u64> {
        self.track(id).map(|track| track.sequencer.position_frame())
    }

    pub fn track_is_audible(&self, id: TrackId) -> bool {
        let any_soloed = self.tracks.iter().any(|track| track.soloed);
        self.track(id)
            .is_some_and(|track| track.is_audible(any_soloed))
    }

    pub fn track_active_revision(&self, id: TrackId) -> Option<u64> {
        self.track(id).map(|track| track.active_revision)
    }

    pub fn queue_pattern_revision(
        &mut self,
        id: TrackId,
        change: QuantizedChange<CompiledPatternRevision>,
    ) -> Result<(), String> {
        let track = self.track_mut(id)?;
        if change.value.revision <= track.active_revision {
            return Err("pattern revision must be newer than the active revision".into());
        }
        if track
            .pending_pattern
            .is_some_and(|pending| pending.value.revision >= change.value.revision)
        {
            return Err("pattern revision must be newer than the queued revision".into());
        }
        track.pending_pattern = Some(change);
        Ok(())
    }

    pub fn set_gain(&mut self, id: TrackId, gain: f32) -> Result<(), String> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err("track gain must be finite and between 0.0 and 2.0".into());
        }
        let track = self.track_mut(id)?;
        track.gain.set_target(gain, track.smoothing_frames);
        Ok(())
    }

    pub fn set_pan(&mut self, id: TrackId, pan: f32) -> Result<(), String> {
        if !pan.is_finite() || !(-1.0..=1.0).contains(&pan) {
            return Err("track pan must be finite and between -1.0 and 1.0".into());
        }
        let track = self.track_mut(id)?;
        track.pan.set_target(pan, track.smoothing_frames);
        Ok(())
    }

    pub fn set_mute(&mut self, id: TrackId, muted: bool) -> Result<(), String> {
        self.track_mut(id)?.muted = muted;
        Ok(())
    }

    pub fn set_solo(&mut self, id: TrackId, soloed: bool) -> Result<(), String> {
        self.track_mut(id)?.soloed = soloed;
        Ok(())
    }

    pub fn is_playing(&self) -> bool {
        self.tracks
            .first()
            .is_some_and(|track| track.sequencer.is_playing())
    }

    pub fn set_playing_all(&mut self, playing: bool) {
        for track in &mut self.tracks {
            if track.sequencer.is_playing() && !playing {
                track.synth.handle(EngineCommand::Panic);
            }
            track.sequencer.set_playing(playing);
        }
    }

    pub fn toggle_playing_all(&mut self) -> bool {
        let playing = !self.is_playing();
        self.set_playing_all(playing);
        playing
    }

    pub fn restart_all(&mut self) {
        for track in &mut self.tracks {
            track.synth.handle(EngineCommand::Panic);
            track.sequencer.restart();
        }
    }

    pub fn panic_all(&mut self) {
        for track in &mut self.tracks {
            track.synth.handle(EngineCommand::Panic);
        }
    }

    fn track(&self, id: TrackId) -> Option<&RealtimeTrack> {
        self.tracks.iter().find(|track| track.id == id)
    }

    fn track_mut(&mut self, id: TrackId) -> Result<&mut RealtimeTrack, String> {
        self.tracks
            .iter_mut()
            .find(|track| track.id == id)
            .ok_or_else(|| format!("track id {} does not exist", id.0))
    }
}

fn validate_track_definitions(definitions: &[TrackDefinition]) -> Result<(), String> {
    if definitions.is_empty() {
        return Err("multi-track engine requires at least one track".into());
    }
    if definitions.len() > MAX_REALTIME_TRACKS {
        return Err(format!(
            "multi-track engine supports at most {MAX_REALTIME_TRACKS} tracks"
        ));
    }

    let mut ids = HashSet::with_capacity(definitions.len());
    for definition in definitions {
        definition.validate()?;
        if !ids.insert(definition.id) {
            return Err(format!("duplicate track id: {}", definition.id.0));
        }
    }
    Ok(())
}
