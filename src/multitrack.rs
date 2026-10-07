use crate::effects::{
    check_effect_memory, validate_track_inserts, EffectConfig, EffectRack, EffectsConfig,
    DEFAULT_EFFECT_MEMORY_BUDGET, MAX_MASTER_INSERTS, MAX_SEND_BUSES, MAX_TRACK_INSERTS,
};
use crate::scenes::{
    bar_at_or_after, bar_frame, validate_scenes, ChainId, PatternId, Scene, SceneChain, SceneId,
    CHAIN_BEATS_PER_BAR,
};
use crate::{
    ChannelStrip, CompiledPattern, CompiledPatternRevision, CompiledSamplePlayback,
    CompiledSynthPatch, EffectLocation, EffectParamId, EffectSlotId, EngineCommand, LiveSequencer,
    LockBoundary, LockTarget, Oscillator, ParameterLock, Pattern, QuantizedChange, RealtimeSampler,
    RealtimeSynth, SampleAssetBank, SampleLockOverrides, SampleParamId, SampleSettings,
    SmoothedParam, SynthParamId, SynthParamValue, SynthPatch, TrackParamId, MAX_LOCKS_PER_STEP,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fmt,
    fs::{self, File},
    io::Write,
    path::Path,
};

pub const MAX_REALTIME_TRACKS: usize = 16;
/// Patterns a track may own, including its primary pattern (ID 0).
pub const MAX_TRACK_PATTERNS: usize = 16;
pub const MULTITRACK_PROJECT_SCHEMA_VERSION: u32 = 3;
/// Oldest schema still accepted on load. Version 3 only adds optional
/// fields, so version 2 files load unchanged.
pub const MIN_MULTITRACK_PROJECT_SCHEMA_VERSION: u32 = 2;
/// The primary `pattern` of every track.
pub const PRIMARY_PATTERN_ID: PatternId = PatternId(0);

const PEAK_DECAY: f32 = 0.9995;

fn one() -> f32 {
    1.0
}
fn is_one(value: &f32) -> bool {
    *value == 1.0
}
fn is_zero(value: &f32) -> bool {
    *value == 0.0
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiTrackProject {
    pub schema_version: u32,
    pub revision: u64,
    pub seed: u64,
    pub bpm: f64,
    pub steps_per_beat: u8,
    pub tracks: Vec<TrackDefinition>,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub master_gain: f32,
    #[serde(default, skip_serializing_if = "EffectsConfig::is_empty")]
    pub effects: EffectsConfig,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenes: Vec<Scene>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chains: Vec<SceneChain>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub midi_mappings: Vec<crate::midi_learn::MidiMapping>,
}

impl MultiTrackProject {
    /// A project with the given tracks and no scenes, effects or mappings.
    pub fn new(seed: u64, bpm: f64, steps_per_beat: u8, tracks: Vec<TrackDefinition>) -> Self {
        Self {
            schema_version: MULTITRACK_PROJECT_SCHEMA_VERSION,
            revision: 1,
            seed,
            bpm,
            steps_per_beat,
            tracks,
            master_gain: 1.0,
            effects: EffectsConfig::default(),
            scenes: Vec::new(),
            chains: Vec::new(),
            midi_mappings: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !(MIN_MULTITRACK_PROJECT_SCHEMA_VERSION..=MULTITRACK_PROJECT_SCHEMA_VERSION)
            .contains(&self.schema_version)
        {
            return Err(format!(
                "unsupported multitrack project schema {}; expected {} to {}",
                self.schema_version,
                MIN_MULTITRACK_PROJECT_SCHEMA_VERSION,
                MULTITRACK_PROJECT_SCHEMA_VERSION
            ));
        }
        if !self.bpm.is_finite() || !(20.0..=400.0).contains(&self.bpm) {
            return Err("project bpm must be finite and between 20 and 400".into());
        }
        if !(1..=64).contains(&self.steps_per_beat) {
            return Err("project steps_per_beat must be between 1 and 64".into());
        }
        if !self.master_gain.is_finite() || !(0.0..=2.0).contains(&self.master_gain) {
            return Err("project master_gain must be finite and between 0.0 and 2.0".into());
        }
        validate_track_definitions(&self.tracks)?;
        self.effects.validate()?;
        validate_scenes(&self.scenes, &self.chains, |track, pattern| {
            self.tracks
                .iter()
                .find(|definition| definition.id == track)
                .is_some_and(|definition| definition.pattern_by_id(pattern).is_some())
        })?;
        for mapping in &self.midi_mappings {
            mapping.validate()?;
        }
        Ok(())
    }

    /// Effect memory required at `sample_rate`, checked against the default
    /// budget before audio starts.
    pub fn check_effect_memory(&self, sample_rate: u32) -> Result<(), String> {
        let tracks: usize = self
            .tracks
            .iter()
            .flat_map(|track| &track.inserts)
            .map(|effect| effect.required_memory_bytes(sample_rate))
            .sum();
        check_effect_memory(
            tracks + self.effects.required_memory_bytes(sample_rate),
            DEFAULT_EFFECT_MEMORY_BUDGET,
        )
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

    /// Settings of every sample track, in track order.
    pub fn sample_settings(&self) -> impl Iterator<Item = &SampleSettings> {
        self.tracks.iter().filter_map(|track| track.sample.as_ref())
    }

    /// Decode every sample the project references, resolving relative paths
    /// against `base_dir` (normally the project file's folder). Fails before
    /// activation with a report of every missing or invalid file.
    pub fn load_sample_assets(
        &self,
        base_dir: &Path,
        budget_bytes: usize,
    ) -> Result<SampleAssetBank, String> {
        SampleAssetBank::load_all(self.sample_settings(), base_dir, budget_bytes)
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
    SetSend { track: TrackId, bus: u8, level: f32 },
    Panic { track: Option<TrackId> },
    RestartAll,
}

/// Allocation-free error for engine operations that may run on the audio
/// thread.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineError {
    TrackNotFound(u16),
    PatternNotFound(u16),
    SceneNotFound(u16),
    ChainNotFound(u16),
    InvalidValue(&'static str),
    StaleRevision,
    WrongTrackKind,
    InvalidSlot,
    TrackLimit,
    DuplicateTrack(u16),
    RateMismatch,
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TrackNotFound(id) => write!(f, "track id {id} does not exist"),
            Self::PatternNotFound(id) => write!(f, "pattern id {id} does not exist on this track"),
            Self::SceneNotFound(id) => write!(f, "scene id {id} does not exist"),
            Self::ChainNotFound(id) => write!(f, "chain id {id} does not exist"),
            Self::InvalidValue(message) => f.write_str(message),
            Self::StaleRevision => {
                f.write_str("pattern revision must be newer than the active and queued revisions")
            }
            Self::WrongTrackKind => f.write_str("operation does not apply to this track kind"),
            Self::InvalidSlot => f.write_str("effect slot or parameter does not exist"),
            Self::TrackLimit => write!(
                f,
                "the engine already has the maximum of {MAX_REALTIME_TRACKS} tracks"
            ),
            Self::DuplicateTrack(id) => write!(f, "track id {id} already exists"),
            Self::RateMismatch => f.write_str("compiled data uses a different sample rate"),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<EngineError> for String {
    fn from(error: EngineError) -> Self {
        error.to_string()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
pub struct TrackId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Synth,
    Sample,
    ExternalMidi,
}

/// An additional pattern a track can switch to through scenes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryPattern {
    pub id: PatternId,
    pub pattern: Pattern,
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
    /// Primary pattern; its pattern ID is always 0.
    pub pattern: Pattern,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub synth_patch: Option<SynthPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<SampleSettings>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pattern_library: Vec<LibraryPattern>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inserts: Vec<EffectConfig>,
    /// Post-fader send level to send bus A.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub send_a: f32,
    /// Post-fader send level to send bus B.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub send_b: f32,
}

impl TrackDefinition {
    /// A centred, unmuted track with no effects or alternate patterns.
    pub fn new(id: TrackId, name: impl Into<String>, kind: TrackKind, pattern: Pattern) -> Self {
        Self {
            id,
            name: name.into(),
            kind,
            gain: 1.0,
            pan: 0.0,
            muted: false,
            soloed: false,
            pattern,
            synth_patch: None,
            sample: None,
            pattern_library: Vec::new(),
            inserts: Vec::new(),
            send_a: 0.0,
            send_b: 0.0,
        }
    }

    /// The primary pattern for ID 0, otherwise a library pattern.
    pub fn pattern_by_id(&self, id: PatternId) -> Option<&Pattern> {
        if id == PRIMARY_PATTERN_ID {
            return Some(&self.pattern);
        }
        self.pattern_library
            .iter()
            .find(|entry| entry.id == id)
            .map(|entry| &entry.pattern)
    }

    /// Every pattern ID the track owns, primary first.
    pub fn pattern_ids(&self) -> impl Iterator<Item = PatternId> + '_ {
        std::iter::once(PRIMARY_PATTERN_ID).chain(self.pattern_library.iter().map(|entry| entry.id))
    }

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
        for (label, level) in [("send_a", self.send_a), ("send_b", self.send_b)] {
            if !level.is_finite() || !(0.0..=1.0).contains(&level) {
                return Err(format!(
                    "track {label} must be finite and between 0.0 and 1.0"
                ));
            }
        }
        if let Some(patch) = self.synth_patch {
            if self.kind != TrackKind::Synth {
                return Err("synth patches are only supported on synth tracks".into());
            }
            // File validation uses the absolute supported cutoff ceiling.
            // Engine construction validates again at the actual device rate.
            patch.validate(48_000.0)?;
        }
        match (&self.sample, self.kind) {
            (Some(sample), TrackKind::Sample) => sample.validate()?,
            (None, TrackKind::Sample) => {
                return Err(format!(
                    "sample track {} needs a \"sample\" object with at least path and mode",
                    self.id.0
                ))
            }
            (Some(_), _) => {
                return Err("sample settings are only supported on sample tracks".into())
            }
            (None, _) => {}
        }
        validate_track_inserts(&self.inserts)
            .map_err(|error| format!("track {} inserts: {error}", self.id.0))?;
        if self.pattern_library.len() >= MAX_TRACK_PATTERNS {
            return Err(format!(
                "track {} may own at most {MAX_TRACK_PATTERNS} patterns",
                self.id.0
            ));
        }
        let mut ids = HashSet::from([PRIMARY_PATTERN_ID]);
        for entry in &self.pattern_library {
            if !ids.insert(entry.id) {
                return Err(format!(
                    "track {} pattern id {} is duplicated (0 is the primary pattern)",
                    self.id.0, entry.id.0
                ));
            }
            entry
                .pattern
                .validate()
                .map_err(|error| format!("track {} pattern {}: {error}", self.id.0, entry.id.0))?;
        }
        self.pattern.validate()
    }
}

/// Decoded samples plus the folder their relative paths were resolved from,
/// handed to the engine so sample tracks can find their audio.
#[derive(Debug, Clone, Copy)]
pub struct SampleContext<'a> {
    pub assets: &'a SampleAssetBank,
    pub base_dir: &'a Path,
}

#[derive(Debug, Clone)]
enum TrackInstrument {
    Synth(RealtimeSynth),
    Sample(RealtimeSampler),
}

impl TrackInstrument {
    fn handle(&mut self, command: EngineCommand) {
        match self {
            Self::Synth(synth) => synth.handle(command),
            Self::Sample(sampler) => sampler.handle(command),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ActiveLock {
    lock: ParameterLock,
    /// Base value of a locked effect parameter, restored when the lock ends.
    effect_base: f32,
}

const NO_LOCK: ActiveLock = ActiveLock {
    lock: ParameterLock {
        target: LockTarget::Track(TrackParamId::Gain),
        value: 0.0,
    },
    effect_base: 0.0,
};

#[derive(Debug, Clone, Copy)]
struct PendingRevision {
    library_index: usize,
    change: QuantizedChange<CompiledPatternRevision>,
}

#[derive(Debug, Clone)]
struct RealtimeTrack {
    id: TrackId,
    sample_rate: u32,
    sequencer: LiveSequencer,
    instrument: TrackInstrument,
    library_ids: Vec<PatternId>,
    library: Vec<CompiledPattern>,
    library_revisions: Vec<u64>,
    active_index: usize,
    gain: SmoothedParam,
    pan: SmoothedParam,
    sends: [SmoothedParam; MAX_SEND_BUSES],
    base_gain: f32,
    base_pan: f32,
    base_sends: [f32; MAX_SEND_BUSES],
    base_patch: Option<SynthPatch>,
    muted: bool,
    soloed: bool,
    smoothing_frames: u32,
    pending_pattern: Option<PendingRevision>,
    inserts: EffectRack,
    locks: [ActiveLock; MAX_LOCKS_PER_STEP],
    lock_len: usize,
    last_output: (f32, f32),
    peak: f32,
}

/// A track built on the control thread, ready to be moved into a running
/// engine with [`MultiTrackEngine::add_track`].
#[derive(Debug, Clone)]
pub struct PreparedTrack(RealtimeTrack);

impl PreparedTrack {
    pub fn new(
        sample_rate: u32,
        bpm: f64,
        steps_per_beat: u32,
        project_seed: u64,
        polyphony: usize,
        definition: TrackDefinition,
        samples: Option<SampleContext<'_>>,
    ) -> Result<Self, String> {
        RealtimeTrack::new(
            sample_rate,
            bpm,
            steps_per_beat,
            project_seed,
            polyphony,
            definition,
            samples,
        )
        .map(Self)
    }

    pub fn id(&self) -> TrackId {
        self.0.id
    }
}

impl RealtimeTrack {
    fn new(
        sample_rate: u32,
        bpm: f64,
        steps_per_beat: u32,
        project_seed: u64,
        polyphony: usize,
        definition: TrackDefinition,
        samples: Option<SampleContext<'_>>,
    ) -> Result<Self, String> {
        definition.validate()?;
        let instrument = match (definition.kind, definition.sample.as_ref()) {
            (TrackKind::Synth, _) => {
                let patch = definition
                    .synth_patch
                    .unwrap_or_else(|| SynthPatch::legacy(Oscillator::Saw));
                TrackInstrument::Synth(RealtimeSynth::new_with_patch(
                    sample_rate as f32,
                    patch,
                    polyphony,
                )?)
            }
            (TrackKind::Sample, Some(settings)) => {
                let base_dir = samples.map_or(Path::new("."), |samples| samples.base_dir);
                let path = settings.resolve_path(base_dir);
                let asset = samples
                    .and_then(|samples| {
                        let bank = samples.assets;
                        bank.id_for_path(&path).and_then(|id| bank.get(id))
                    })
                    .ok_or_else(|| {
                        format!(
                            "sample track {} references {} which has not been loaded",
                            definition.id.0,
                            path.display()
                        )
                    })?;
                let playback = CompiledSamplePlayback::new(asset, settings, sample_rate)
                    .map_err(|error| format!("sample track {}: {error}", definition.id.0))?;
                TrackInstrument::Sample(RealtimeSampler::new(playback))
            }
            (kind, _) => {
                return Err(format!(
                    "track {:?} uses {:?}, which is not implemented by the real-time engine yet",
                    definition.id, kind
                ))
            }
        };
        let base_patch = match &instrument {
            TrackInstrument::Synth(synth) => Some(synth.patch()),
            TrackInstrument::Sample(_) => None,
        };

        let mut library_ids = Vec::with_capacity(MAX_TRACK_PATTERNS);
        let mut library = Vec::with_capacity(MAX_TRACK_PATTERNS);
        for id in definition.pattern_ids() {
            let pattern = definition
                .pattern_by_id(id)
                .expect("pattern_ids only yields owned patterns");
            library_ids.push(id);
            library.push(CompiledPattern::from_pattern(pattern)?);
        }
        let library_revisions = vec![0; library.len()];

        let sequencer = LiveSequencer::new(
            sample_rate,
            bpm,
            steps_per_beat,
            project_seed,
            definition.pattern,
        )?;
        let inserts = EffectRack::new(&definition.inserts, MAX_TRACK_INSERTS, sample_rate, bpm)
            .map_err(|error| format!("track {} inserts: {error}", definition.id.0))?;
        let smoothing_frames = (sample_rate / 200).max(1);
        let base_sends = [definition.send_a, definition.send_b];

        Ok(Self {
            id: definition.id,
            sample_rate,
            sequencer,
            instrument,
            library_ids,
            library,
            library_revisions,
            active_index: 0,
            gain: SmoothedParam::new(definition.gain),
            pan: SmoothedParam::new(definition.pan),
            sends: base_sends.map(SmoothedParam::new),
            base_gain: definition.gain,
            base_pan: definition.pan,
            base_sends,
            base_patch,
            muted: definition.muted,
            soloed: definition.soloed,
            smoothing_frames,
            pending_pattern: None,
            inserts,
            locks: [NO_LOCK; MAX_LOCKS_PER_STEP],
            lock_len: 0,
            last_output: (0.0, 0.0),
            peak: 0.0,
        })
    }

    fn is_audible(&self, any_soloed: bool) -> bool {
        !self.muted && (!any_soloed || self.soloed)
    }

    fn library_index(&self, id: PatternId) -> Option<usize> {
        self.library_ids
            .iter()
            .position(|candidate| *candidate == id)
    }

    fn active_locks(&self) -> &[ActiveLock] {
        &self.locks[..self.lock_len]
    }

    fn locked_value(&self, target: LockTarget) -> Option<f32> {
        self.active_locks()
            .iter()
            .find(|active| active.lock.target == target)
            .map(|active| active.lock.value)
    }

    /// Switch to library pattern `index` at the current frame. Old locks
    /// never leak across a pattern replacement.
    fn activate_library_pattern(&mut self, index: usize) {
        // A revision queued for the pattern being left must not be lost:
        // commit it to the library now (it is no longer audible).
        if let Some(pending) = self.pending_pattern {
            if pending.library_index != index {
                self.pending_pattern = None;
                self.library[pending.library_index] = pending.change.value.pattern;
                self.library_revisions[pending.library_index] = pending.change.value.revision;
            }
        }
        // Release the old pattern's notes through their envelopes rather
        // than hard-stopping them, which would click.
        let instrument = &mut self.instrument;
        self.sequencer
            .release_pending(|command| instrument.handle(command));
        self.sequencer.replace_compiled_pattern(self.library[index]);
        self.active_index = index;
        self.set_locks(&[]);
    }

    /// Apply a queued revision immediately (used when the timeline it was
    /// quantized against is discarded, e.g. on restart).
    fn commit_pending_now(&mut self) {
        if let Some(pending) = self.pending_pattern.take() {
            let index = pending.library_index;
            self.library[index] = pending.change.value.pattern;
            self.library_revisions[index] = pending.change.value.revision;
            if index == self.active_index {
                self.sequencer.replace_compiled_pattern(self.library[index]);
            }
        }
    }

    /// Replace the lock set. Restoration happens before new locks are
    /// applied, so a parameter locked on consecutive steps moves straight
    /// from the old lock value to the new one.
    fn set_locks(&mut self, new_locks: &[ParameterLock]) {
        if self.lock_len == 0 && new_locks.is_empty() {
            return;
        }
        let touched_synth = self
            .active_locks()
            .iter()
            .map(|active| active.lock.target)
            .chain(new_locks.iter().map(|lock| lock.target))
            .any(|target| matches!(target, LockTarget::Synth(_)));

        let mut next = [NO_LOCK; MAX_LOCKS_PER_STEP];
        let mut next_len = 0;
        for lock in new_locks.iter().take(MAX_LOCKS_PER_STEP) {
            let effect_base = match lock.target {
                LockTarget::Effect { slot, param } => self
                    .active_locks()
                    .iter()
                    .find(|active| active.lock.target == lock.target)
                    .map(|active| active.effect_base)
                    .or_else(|| self.inserts.param(slot, param))
                    .unwrap_or(0.0),
                _ => 0.0,
            };
            next[next_len] = ActiveLock {
                lock: *lock,
                effect_base,
            };
            next_len += 1;
        }

        // Restore effect parameters that are no longer locked.
        for index in 0..self.lock_len {
            let active = self.locks[index];
            if let LockTarget::Effect { slot, param } = active.lock.target {
                if !next[..next_len]
                    .iter()
                    .any(|candidate| candidate.lock.target == active.lock.target)
                {
                    self.inserts.set_param(slot, param, active.effect_base);
                }
            }
        }
        self.locks = next;
        self.lock_len = next_len;

        for index in 0..self.lock_len {
            if let LockTarget::Effect { slot, param } = self.locks[index].lock.target {
                self.inserts
                    .set_param(slot, param, self.locks[index].lock.value);
            }
        }
        self.refresh_mixer_targets();
        self.refresh_sample_overrides();
        if touched_synth {
            self.refresh_synth_patch();
        }
    }

    fn refresh_mixer_targets(&mut self) {
        let gain = self
            .locked_value(LockTarget::Track(TrackParamId::Gain))
            .unwrap_or(self.base_gain);
        let pan = self
            .locked_value(LockTarget::Track(TrackParamId::Pan))
            .unwrap_or(self.base_pan);
        self.gain.set_target(gain, self.smoothing_frames);
        self.pan.set_target(pan, self.smoothing_frames);
        for (bus, param) in [TrackParamId::SendA, TrackParamId::SendB]
            .into_iter()
            .enumerate()
        {
            let level = self
                .locked_value(LockTarget::Track(param))
                .unwrap_or(self.base_sends[bus]);
            self.sends[bus].set_target(level, self.smoothing_frames);
        }
    }

    fn refresh_sample_overrides(&mut self) {
        let overrides = SampleLockOverrides {
            pitch_semitones: self
                .locked_value(LockTarget::Sample(SampleParamId::Pitch))
                .unwrap_or(0.0),
            gain: self
                .locked_value(LockTarget::Sample(SampleParamId::Gain))
                .unwrap_or(1.0),
            reverse: self
                .locked_value(LockTarget::Sample(SampleParamId::Reverse))
                .map(|value| value >= 0.5),
        };
        if let TrackInstrument::Sample(sampler) = &mut self.instrument {
            sampler.set_lock_overrides(overrides);
        }
    }

    /// Apply base patch plus synth locks. Lock values were validated against
    /// the parameter descriptors at pattern compile time; they are clamped
    /// here again so the patch always validates at the device rate.
    fn refresh_synth_patch(&mut self) {
        let (Some(base), TrackInstrument::Synth(synth)) = (self.base_patch, &mut self.instrument)
        else {
            return;
        };
        let sample_rate = self.sample_rate as f32;
        let mut effective = base;
        for active in &self.locks[..self.lock_len] {
            if let LockTarget::Synth(param) = active.lock.target {
                effective = apply_synth_lock(effective, param, active.lock.value, sample_rate);
            }
        }
        if let Ok(compiled) = CompiledSynthPatch::new(sample_rate, effective) {
            synth.apply_patch(compiled);
        }
    }

    fn apply_lock_boundary(&mut self, boundary: LockBoundary) {
        let mut buffer = [NO_LOCK.lock; MAX_LOCKS_PER_STEP];
        let mut len = 0;
        if boundary.triggered {
            for lock in self
                .sequencer
                .step_locks(usize::from(boundary.step_index))
                .iter()
                .take(MAX_LOCKS_PER_STEP)
            {
                buffer[len] = *lock;
                len += 1;
            }
        }
        self.set_locks(&buffer[..len]);
    }

    fn apply_pending_revision(&mut self) {
        let Some(pending) = self.pending_pattern else {
            return;
        };
        if pending.change.apply_at_frame > self.sequencer.position_frame() {
            return;
        }
        self.pending_pattern = None;
        let index = pending.library_index;
        self.library[index] = pending.change.value.pattern;
        self.library_revisions[index] = pending.change.value.revision;
        if index == self.active_index {
            self.activate_library_pattern(index);
        }
    }

    /// Render one frame: returns the post-fader stereo output and the
    /// post-fader send contributions.
    fn next_stereo_frame(
        &mut self,
        any_soloed: bool,
        commands: &mut Vec<EngineCommand>,
    ) -> ((f32, f32), [f32; MAX_SEND_BUSES]) {
        self.apply_pending_revision();

        let mut boundary = None;
        self.sequencer
            .fill_commands_with_locks(commands, &mut boundary);
        if let Some(boundary) = boundary {
            self.apply_lock_boundary(boundary);
        }
        for command in commands.drain(..) {
            self.instrument.handle(command);
        }

        let gain = self.gain.next_value();
        let pan = self.pan.next_value();
        let send_levels = [self.sends[0].next_value(), self.sends[1].next_value()];
        let strip = ChannelStrip {
            gain,
            pan,
            muted: false,
        };
        let audible = self.is_audible(any_soloed);
        // Instruments (and inserts) always render so voices and effect tails
        // age while muted.
        let output = match &mut self.instrument {
            TrackInstrument::Synth(synth) => {
                let mono = synth.next_sample();
                if self.inserts.is_empty() {
                    if audible {
                        strip.process_mono(mono)
                    } else {
                        (0.0, 0.0)
                    }
                } else {
                    let (left, right) = self.inserts.process(mono, mono);
                    if audible {
                        strip.process_stereo(left, right)
                    } else {
                        (0.0, 0.0)
                    }
                }
            }
            TrackInstrument::Sample(sampler) => {
                let (mut left, mut right) = sampler.next_stereo_frame();
                if !self.inserts.is_empty() {
                    (left, right) = self.inserts.process(left, right);
                }
                if audible {
                    strip.process_stereo(left, right)
                } else {
                    (0.0, 0.0)
                }
            }
        };
        self.last_output = output;
        let level = output.0.abs().max(output.1.abs());
        self.peak = if level > self.peak {
            level
        } else {
            self.peak * PEAK_DECAY
        };
        let sends = send_levels.map(|level| level.clamp(0.0, 1.0));
        (output, sends)
    }
}

fn apply_synth_lock(
    patch: SynthPatch,
    param: SynthParamId,
    value: f32,
    sample_rate: f32,
) -> SynthPatch {
    let descriptor = crate::params::synth_param_descriptor(param);
    let mut value = value.clamp(descriptor.min, descriptor.max);
    if param == SynthParamId::FilterCutoff {
        value = crate::clamp_cutoff(value, sample_rate);
    }
    if matches!(param, SynthParamId::Octave | SynthParamId::Semitone) {
        value = value.round();
    }
    patch
        .with_parameter(param, SynthParamValue::Number(value), sample_rate)
        .unwrap_or(patch)
}

#[derive(Debug, Clone)]
struct SceneEntry {
    track_index: usize,
    library_index: usize,
    muted: Option<bool>,
    gain: Option<f32>,
}

#[derive(Debug, Clone)]
struct CompiledScene {
    id: SceneId,
    entries: Vec<SceneEntry>,
}

#[derive(Debug, Clone)]
struct CompiledChain {
    id: ChainId,
    /// (scene index, bars)
    steps: Vec<(usize, u16)>,
    loop_chain: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChainRun {
    chain: usize,
    step: usize,
    /// Bar index at which the next chain step launches.
    next_bar: u64,
    paused: bool,
}

/// Per-track state for telemetry and status displays.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackStatus {
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
    pub lock_count: usize,
}

/// Observable chain state for status displays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainStatus {
    pub chain: ChainId,
    pub step: usize,
    pub steps: usize,
    pub next_change_frame: u64,
    pub paused: bool,
}

#[derive(Debug, Clone)]
struct SendBus {
    rack: EffectRack,
    return_gain: f32,
}

#[derive(Debug, Clone)]
pub struct MultiTrackEngine {
    sample_rate: u32,
    bpm: f64,
    steps_per_beat: u32,
    tracks: Vec<RealtimeTrack>,
    command_buffer: Vec<EngineCommand>,
    send_buses: Vec<SendBus>,
    master_rack: EffectRack,
    master_gain: SmoothedParam,
    smoothing_frames: u32,
    scenes: Vec<CompiledScene>,
    chains: Vec<CompiledChain>,
    pending_scene: Option<(usize, u64)>,
    active_scene: Option<SceneId>,
    chain_run: Option<ChainRun>,
    frames_per_bar: f64,
    master_peak: f32,
    last_master: (f32, f32),
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
        Self::with_sample_assets(
            sample_rate,
            bpm,
            steps_per_beat,
            project_seed,
            polyphony_per_track,
            definitions,
            None,
        )
    }

    /// Build an engine that can also play sample tracks. The context's bank
    /// must already hold every sample the definitions reference (see
    /// [`MultiTrackProject::load_sample_assets`]), and its `base_dir` must be
    /// the folder the bank resolved relative paths against.
    pub fn with_sample_assets(
        sample_rate: u32,
        bpm: f64,
        steps_per_beat: u32,
        project_seed: u64,
        polyphony_per_track: usize,
        definitions: Vec<TrackDefinition>,
        samples: Option<SampleContext<'_>>,
    ) -> Result<Self, String> {
        validate_track_definitions(&definitions)?;
        if sample_rate == 0 {
            return Err("sample rate must be greater than zero".into());
        }

        let mut tracks = Vec::with_capacity(MAX_REALTIME_TRACKS);
        for definition in definitions {
            tracks.push(RealtimeTrack::new(
                sample_rate,
                bpm,
                steps_per_beat,
                project_seed,
                polyphony_per_track,
                definition,
                samples,
            )?);
        }

        let frames_per_bar = f64::from(sample_rate) * 60.0 / bpm * f64::from(CHAIN_BEATS_PER_BAR);
        Ok(Self {
            sample_rate,
            bpm,
            steps_per_beat,
            tracks,
            command_buffer: Vec::with_capacity(LiveSequencer::MAX_COMMANDS_PER_FRAME),
            send_buses: Vec::new(),
            master_rack: EffectRack::default(),
            master_gain: SmoothedParam::new(1.0),
            smoothing_frames: (sample_rate / 200).max(1),
            scenes: Vec::new(),
            chains: Vec::new(),
            pending_scene: None,
            active_scene: None,
            chain_run: None,
            frames_per_bar,
            master_peak: 0.0,
            last_master: (0.0, 0.0),
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

    /// Build the full engine for a project: tracks, effects, scenes and
    /// chains. All allocation happens here, before audio starts.
    pub fn from_project(
        sample_rate: u32,
        polyphony_per_track: usize,
        project: &MultiTrackProject,
        samples: Option<SampleContext<'_>>,
    ) -> Result<Self, String> {
        project.validate()?;
        project.check_effect_memory(sample_rate)?;
        let mut engine = Self::with_sample_assets(
            sample_rate,
            project.bpm,
            u32::from(project.steps_per_beat),
            project.seed,
            polyphony_per_track,
            project.tracks.clone(),
            samples,
        )?;
        engine.master_gain = SmoothedParam::new(project.master_gain);
        engine.master_rack = EffectRack::new(
            &project.effects.master,
            MAX_MASTER_INSERTS,
            sample_rate,
            project.bpm,
        )?;
        let mut buses = Vec::with_capacity(MAX_SEND_BUSES);
        for index in 0..MAX_SEND_BUSES {
            let bus = match project.effects.send_buses.get(index) {
                Some(config) => SendBus {
                    rack: EffectRack::new(
                        &config.effects,
                        crate::effects::MAX_SEND_EFFECTS,
                        sample_rate,
                        project.bpm,
                    )?,
                    return_gain: config.return_gain,
                },
                // Unconfigured buses pass their input straight to the master.
                None => SendBus {
                    rack: EffectRack::default(),
                    return_gain: 1.0,
                },
            };
            buses.push(bus);
        }
        engine.send_buses = buses;
        engine.set_scenes(&project.scenes, &project.chains)?;
        Ok(engine)
    }

    /// Compile scenes and chains against the engine's tracks. Control-side
    /// only: it allocates.
    pub fn set_scenes(&mut self, scenes: &[Scene], chains: &[SceneChain]) -> Result<(), String> {
        validate_scenes(scenes, chains, |track, pattern| {
            self.tracks
                .iter()
                .find(|candidate| candidate.id == track)
                .is_some_and(|candidate| candidate.library_index(pattern).is_some())
        })?;
        let mut compiled = Vec::with_capacity(scenes.len());
        for scene in scenes {
            let mut entries = Vec::with_capacity(scene.track_states.len());
            for state in &scene.track_states {
                let track_index = self
                    .tracks
                    .iter()
                    .position(|track| track.id == state.track_id)
                    .ok_or_else(|| {
                        format!("scene references unknown track {}", state.track_id.0)
                    })?;
                let library_index = self.tracks[track_index]
                    .library_index(state.pattern_id)
                    .ok_or_else(|| {
                        format!("scene references unknown pattern {}", state.pattern_id.0)
                    })?;
                entries.push(SceneEntry {
                    track_index,
                    library_index,
                    muted: state.muted,
                    gain: state.gain,
                });
            }
            compiled.push(CompiledScene {
                id: scene.id,
                entries,
            });
        }
        let mut compiled_chains = Vec::with_capacity(chains.len());
        for chain in chains {
            let steps = chain
                .steps
                .iter()
                .map(|step| {
                    compiled
                        .iter()
                        .position(|scene| scene.id == step.scene_id)
                        .map(|index| (index, step.repeats))
                        .ok_or_else(|| {
                            format!("chain references unknown scene {}", step.scene_id.0)
                        })
                })
                .collect::<Result<Vec<_>, _>>()?;
            compiled_chains.push(CompiledChain {
                id: chain.id,
                steps,
                loop_chain: chain.loop_chain,
            });
        }
        self.scenes = compiled;
        self.chains = compiled_chains;
        self.pending_scene = None;
        self.chain_run = None;
        Ok(())
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn bpm(&self) -> f64 {
        self.bpm
    }

    pub fn frames_per_bar(&self) -> f64 {
        self.frames_per_bar
    }

    /// Infallible bounded callback path: false means missing track or rate mismatch.
    pub fn apply_synth_patch(&mut self, id: TrackId, patch: CompiledSynthPatch) -> bool {
        let Some(track) = self.tracks.iter_mut().find(|track| track.id == id) else {
            return false;
        };
        let TrackInstrument::Synth(synth) = &mut track.instrument else {
            return false;
        };
        let has_synth_locks = track.locks[..track.lock_len]
            .iter()
            .any(|active| matches!(active.lock.target, LockTarget::Synth(_)));
        if !has_synth_locks {
            if !synth.apply_patch(patch) {
                return false;
            }
            track.base_patch = Some(synth.patch());
            return true;
        }
        // A base change while locked becomes the restoration target but does
        // not replace the sounding locked values.
        if patch.sample_rate() != track.sample_rate as f32 {
            return false;
        }
        track.base_patch = Some(patch.patch());
        track.refresh_synth_patch();
        true
    }

    pub fn apply_command(&mut self, command: TrackCommand) -> Result<(), EngineError> {
        match command {
            TrackCommand::SetGain { track, gain } => self.set_gain(track, gain),
            TrackCommand::SetPan { track, pan } => self.set_pan(track, pan),
            TrackCommand::SetMute { track, muted } => self.set_mute(track, muted),
            TrackCommand::SetSolo { track, soloed } => self.set_solo(track, soloed),
            TrackCommand::SetSend { track, bus, level } => self.set_send(track, bus, level),
            TrackCommand::Panic { track: Some(track) } => {
                let track = self.track_mut(track)?;
                track.instrument.handle(EngineCommand::Panic);
                track.set_locks(&[]);
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
        self.advance_scene_clock();
        let any_soloed = self.tracks.iter().any(|track| track.soloed);
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;
        let mut send_inputs = [(0.0_f32, 0.0_f32); MAX_SEND_BUSES];

        for track in &mut self.tracks {
            let ((track_left, track_right), sends) =
                track.next_stereo_frame(any_soloed, &mut self.command_buffer);
            left += track_left;
            right += track_right;
            for (input, level) in send_inputs.iter_mut().zip(sends) {
                input.0 += track_left * level;
                input.1 += track_right * level;
            }
        }

        for (bus, (send_left, send_right)) in self.send_buses.iter_mut().zip(send_inputs) {
            let (wet_left, wet_right) = bus.rack.process(send_left, send_right);
            left += wet_left * bus.return_gain;
            right += wet_right * bus.return_gain;
        }
        if !self.master_rack.is_empty() {
            (left, right) = self.master_rack.process(left, right);
        }
        let master_gain = self.master_gain.next_value();
        let output = (
            crate::protect_master(left * master_gain),
            crate::protect_master(right * master_gain),
        );
        self.last_master = output;
        let level = output.0.abs().max(output.1.abs());
        self.master_peak = if level > self.master_peak {
            level
        } else {
            self.master_peak * PEAK_DECAY
        };
        output
    }

    /// Launch queued scenes and advance a running chain. Runs once per frame
    /// before any track renders, so every track in a scene changes on the
    /// same frame.
    fn advance_scene_clock(&mut self) {
        let position = self.position_frame();
        if let Some(mut run) = self.chain_run {
            if !run.paused && position >= bar_frame(run.next_bar, self.frames_per_bar) {
                let chain = &self.chains[run.chain];
                run.step += 1;
                if run.step >= chain.steps.len() {
                    if chain.loop_chain {
                        run.step = 0;
                    } else {
                        self.chain_run = None;
                        return self.apply_pending_scene(position);
                    }
                }
                let (scene, bars) = chain.steps[run.step];
                run.next_bar = run.next_bar.saturating_add(u64::from(bars));
                self.chain_run = Some(run);
                self.pending_scene = Some((scene, position));
            }
        }
        self.apply_pending_scene(position);
    }

    fn apply_pending_scene(&mut self, position: u64) {
        let Some((scene_index, frame)) = self.pending_scene else {
            return;
        };
        if frame > position {
            return;
        }
        self.pending_scene = None;
        let scene = &self.scenes[scene_index];
        for entry in &scene.entries {
            let track = &mut self.tracks[entry.track_index];
            if track.active_index != entry.library_index {
                track.activate_library_pattern(entry.library_index);
            }
            if let Some(muted) = entry.muted {
                track.muted = muted;
            }
            if let Some(gain) = entry.gain {
                track.base_gain = gain;
                track.refresh_mixer_targets();
            }
        }
        self.active_scene = Some(scene.id);
    }

    /// Queue a scene at `apply_at_frame`. A manual launch stops a running
    /// chain (v1 policy: no hidden resume).
    pub fn launch_scene(&mut self, id: SceneId, apply_at_frame: u64) -> Result<(), EngineError> {
        let index = self
            .scenes
            .iter()
            .position(|scene| scene.id == id)
            .ok_or(EngineError::SceneNotFound(id.0))?;
        self.chain_run = None;
        self.pending_scene = Some((index, apply_at_frame));
        Ok(())
    }

    /// Launch a scene on the next `boundary` resolved against the engine's
    /// own playhead, so a stale control-side frame can never land a scene
    /// off the grid or skip the downbeat.
    pub fn launch_scene_at(
        &mut self,
        id: SceneId,
        boundary: crate::QuantizeBoundary,
    ) -> Result<u64, EngineError> {
        let frame = crate::next_boundary_frame(
            self.position_frame(),
            self.sample_rate,
            self.bpm,
            self.steps_per_beat,
            boundary,
        )
        .map_err(|_| EngineError::InvalidValue("invalid scene launch boundary"))?;
        self.launch_scene(id, frame)?;
        Ok(frame)
    }

    /// Start a chain on the first bar boundary at or after `from_frame`.
    pub fn start_chain(&mut self, id: ChainId, from_frame: u64) -> Result<u64, EngineError> {
        let index = self
            .chains
            .iter()
            .position(|chain| chain.id == id)
            .ok_or(EngineError::ChainNotFound(id.0))?;
        // Never start in the past: a stale request starts on the next bar.
        let from_frame = from_frame.max(self.position_frame());
        let start_bar = bar_at_or_after(from_frame, self.frames_per_bar);
        let start_frame = bar_frame(start_bar, self.frames_per_bar);
        let (scene, bars) = self.chains[index].steps[0];
        self.pending_scene = Some((scene, start_frame));
        self.chain_run = Some(ChainRun {
            chain: index,
            step: 0,
            next_bar: start_bar.saturating_add(u64::from(bars)),
            paused: false,
        });
        Ok(start_frame)
    }

    pub fn stop_chain(&mut self) {
        self.chain_run = None;
    }

    /// Pause or resume chain advancement; the current scene keeps playing.
    /// Resuming re-anchors the remaining bars of the current step to the
    /// next bar boundary.
    pub fn pause_chain(&mut self, paused: bool) {
        let position = self.position_frame();
        let frames_per_bar = self.frames_per_bar;
        if let Some(run) = self.chain_run.as_mut() {
            if run.paused && !paused {
                let current_bar = bar_at_or_after(position, frames_per_bar);
                run.next_bar = run.next_bar.max(current_bar);
            }
            run.paused = paused;
        }
    }

    pub fn active_scene(&self) -> Option<SceneId> {
        self.active_scene
    }

    /// Queued scene and the frame it activates on.
    pub fn queued_scene(&self) -> Option<(SceneId, u64)> {
        self.pending_scene
            .map(|(index, frame)| (self.scenes[index].id, frame))
    }

    pub fn chain_status(&self) -> Option<ChainStatus> {
        self.chain_run.map(|run| ChainStatus {
            chain: self.chains[run.chain].id,
            step: run.step,
            steps: self.chains[run.chain].steps.len(),
            next_change_frame: bar_frame(run.next_bar, self.frames_per_bar),
            paused: run.paused,
        })
    }

    pub fn track_ids(&self) -> Vec<TrackId> {
        self.tracks.iter().map(|track| track.id).collect()
    }

    pub fn track_count(&self) -> usize {
        self.tracks.len()
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

    pub fn track_is_muted(&self, id: TrackId) -> Option<bool> {
        self.track(id).map(|track| track.muted)
    }

    pub fn track_base_gain(&self, id: TrackId) -> Option<f32> {
        self.track(id).map(|track| track.base_gain)
    }

    /// Current (smoothed) gain including any active lock.
    pub fn track_effective_gain(&self, id: TrackId) -> Option<f32> {
        self.track(id).map(|track| track.gain.target())
    }

    pub fn track_active_revision(&self, id: TrackId) -> Option<u64> {
        self.track(id)
            .map(|track| track.library_revisions[track.active_index])
    }

    pub fn track_active_pattern(&self, id: TrackId) -> Option<PatternId> {
        self.track(id)
            .map(|track| track.library_ids[track.active_index])
    }

    /// Revision queued for the active pattern, if any.
    pub fn track_queued_revision(&self, id: TrackId) -> Option<u64> {
        self.track(id).and_then(|track| {
            track
                .pending_pattern
                .map(|pending| pending.change.value.revision)
        })
    }

    /// Number of parameter locks currently owning parameters on a track.
    pub fn track_lock_count(&self, id: TrackId) -> Option<usize> {
        self.track(id).map(|track| track.lock_len)
    }

    /// Effective synth patch (base plus locks) of a synth track.
    pub fn track_synth_patch(&self, id: TrackId) -> Option<SynthPatch> {
        match self.track(id).map(|track| &track.instrument) {
            Some(TrackInstrument::Synth(synth)) => Some(synth.patch()),
            _ => None,
        }
    }

    /// Base synth patch (without locks) of a synth track.
    pub fn track_base_synth_patch(&self, id: TrackId) -> Option<SynthPatch> {
        self.track(id).and_then(|track| track.base_patch)
    }

    pub fn track_sample_overrides(&self, id: TrackId) -> Option<SampleLockOverrides> {
        match self.track(id).map(|track| &track.instrument) {
            Some(TrackInstrument::Sample(sampler)) => Some(sampler.lock_overrides()),
            _ => None,
        }
    }

    /// Last rendered post-fader stereo frame of a track (resampling source).
    pub fn track_output(&self, id: TrackId) -> Option<(f32, f32)> {
        self.track(id).map(|track| track.last_output)
    }

    /// Last rendered protected master frame.
    pub fn master_output(&self) -> (f32, f32) {
        self.last_master
    }

    /// Decaying peak meters, in track order.
    pub fn track_peaks(&self) -> impl Iterator<Item = (TrackId, f32)> + '_ {
        self.tracks.iter().map(|track| (track.id, track.peak))
    }

    /// Allocation-free status of the track at `index` (engine order).
    pub fn track_status(&self, index: usize) -> Option<TrackStatus> {
        let any_soloed = self.tracks.iter().any(|track| track.soloed);
        self.tracks.get(index).map(|track| TrackStatus {
            id: track.id,
            peak: track.peak,
            muted: track.muted,
            soloed: track.soloed,
            audible: track.is_audible(any_soloed),
            gain: track.base_gain,
            pan: track.base_pan,
            active_pattern: track.library_ids[track.active_index],
            active_revision: track.library_revisions[track.active_index],
            queued_revision: track
                .pending_pattern
                .filter(|pending| pending.library_index == track.active_index)
                .map(|pending| pending.change.value.revision),
            lock_count: track.lock_len,
        })
    }

    pub fn master_peak(&self) -> f32 {
        self.master_peak
    }

    pub fn queue_pattern_revision(
        &mut self,
        id: TrackId,
        change: QuantizedChange<CompiledPatternRevision>,
    ) -> Result<(), EngineError> {
        let pattern = self
            .track(id)
            .map(|track| track.library_ids[track.active_index])
            .ok_or(EngineError::TrackNotFound(id.0))?;
        self.queue_library_revision(id, pattern, change)
    }

    /// Queue a revision for one of a track's patterns. Revisions of the
    /// active pattern wait for their quantized frame; revisions of inactive
    /// patterns update the library immediately with no audible effect.
    pub fn queue_library_revision(
        &mut self,
        id: TrackId,
        pattern: PatternId,
        change: QuantizedChange<CompiledPatternRevision>,
    ) -> Result<(), EngineError> {
        let track = self.track_mut(id)?;
        let index = track
            .library_index(pattern)
            .ok_or(EngineError::PatternNotFound(pattern.0))?;
        if change.value.revision <= track.library_revisions[index] {
            return Err(EngineError::StaleRevision);
        }
        if let Some(pending) = track.pending_pattern {
            if pending.library_index == index
                && pending.change.value.revision >= change.value.revision
            {
                return Err(EngineError::StaleRevision);
            }
        }
        if index != track.active_index {
            track.library[index] = change.value.pattern;
            track.library_revisions[index] = change.value.revision;
            return Ok(());
        }
        track.pending_pattern = Some(PendingRevision {
            library_index: index,
            change,
        });
        Ok(())
    }

    pub fn set_gain(&mut self, id: TrackId, gain: f32) -> Result<(), EngineError> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err(EngineError::InvalidValue(
                "track gain must be finite and between 0.0 and 2.0",
            ));
        }
        let track = self.track_mut(id)?;
        track.base_gain = gain;
        track.refresh_mixer_targets();
        Ok(())
    }

    pub fn set_pan(&mut self, id: TrackId, pan: f32) -> Result<(), EngineError> {
        if !pan.is_finite() || !(-1.0..=1.0).contains(&pan) {
            return Err(EngineError::InvalidValue(
                "track pan must be finite and between -1.0 and 1.0",
            ));
        }
        let track = self.track_mut(id)?;
        track.base_pan = pan;
        track.refresh_mixer_targets();
        Ok(())
    }

    pub fn set_send(&mut self, id: TrackId, bus: u8, level: f32) -> Result<(), EngineError> {
        if usize::from(bus) >= MAX_SEND_BUSES {
            return Err(EngineError::InvalidSlot);
        }
        if !level.is_finite() || !(0.0..=1.0).contains(&level) {
            return Err(EngineError::InvalidValue(
                "send level must be finite and between 0.0 and 1.0",
            ));
        }
        let track = self.track_mut(id)?;
        track.base_sends[usize::from(bus)] = level;
        track.refresh_mixer_targets();
        Ok(())
    }

    pub fn set_mute(&mut self, id: TrackId, muted: bool) -> Result<(), EngineError> {
        self.track_mut(id)?.muted = muted;
        Ok(())
    }

    pub fn set_solo(&mut self, id: TrackId, soloed: bool) -> Result<(), EngineError> {
        self.track_mut(id)?.soloed = soloed;
        Ok(())
    }

    pub fn set_master_gain(&mut self, gain: f32) -> Result<(), EngineError> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err(EngineError::InvalidValue(
                "master gain must be finite and between 0.0 and 2.0",
            ));
        }
        self.master_gain.set_target(gain, self.smoothing_frames);
        Ok(())
    }

    pub fn master_gain(&self) -> f32 {
        self.master_gain.target()
    }

    fn rack_mut(&mut self, location: EffectLocation) -> Result<&mut EffectRack, EngineError> {
        match location {
            EffectLocation::Track(id) => Ok(&mut self.track_mut(id)?.inserts),
            EffectLocation::Send(bus) => self
                .send_buses
                .get_mut(usize::from(bus))
                .map(|bus| &mut bus.rack)
                .ok_or(EngineError::InvalidSlot),
            EffectLocation::Master => Ok(&mut self.master_rack),
        }
    }

    fn rack(&self, location: EffectLocation) -> Option<&EffectRack> {
        match location {
            EffectLocation::Track(id) => self.track(id).map(|track| &track.inserts),
            EffectLocation::Send(bus) => self.send_buses.get(usize::from(bus)).map(|bus| &bus.rack),
            EffectLocation::Master => Some(&self.master_rack),
        }
    }

    /// Set an effect parameter's base value. On a track whose step lock
    /// currently owns the parameter, only the restoration target changes.
    pub fn set_effect_param(
        &mut self,
        location: EffectLocation,
        slot: EffectSlotId,
        param: EffectParamId,
        value: f32,
    ) -> Result<(), EngineError> {
        if let EffectLocation::Track(id) = location {
            let track = self.track_mut(id)?;
            let target = LockTarget::Effect { slot, param };
            if let Some(active) = track.locks[..track.lock_len]
                .iter_mut()
                .find(|active| active.lock.target == target)
            {
                if track.inserts.param(slot, param).is_none() || !value.is_finite() {
                    return Err(EngineError::InvalidSlot);
                }
                active.effect_base = value;
                return Ok(());
            }
        }
        if self.rack_mut(location)?.set_param(slot, param, value) {
            Ok(())
        } else {
            Err(EngineError::InvalidSlot)
        }
    }

    pub fn effect_param(
        &self,
        location: EffectLocation,
        slot: EffectSlotId,
        param: EffectParamId,
    ) -> Option<f32> {
        self.rack(location)?.param(slot, param)
    }

    pub fn effect_kind(
        &self,
        location: EffectLocation,
        slot: EffectSlotId,
    ) -> Option<crate::effects::EffectKind> {
        self.rack(location)?.kind(slot)
    }

    pub fn set_effect_bypass(
        &mut self,
        location: EffectLocation,
        slot: EffectSlotId,
        bypassed: bool,
    ) -> Result<(), EngineError> {
        if self.rack_mut(location)?.set_bypassed(slot, bypassed) {
            Ok(())
        } else {
            Err(EngineError::InvalidSlot)
        }
    }

    /// Add a track prepared on the control thread. The playhead joins the
    /// engine's current frame. On failure the track is handed back so the
    /// caller can drop it off the audio thread.
    // Boxing the error would free heap memory on the audio thread.
    #[allow(clippy::result_large_err)]
    pub fn add_track(&mut self, track: PreparedTrack) -> Result<(), (EngineError, PreparedTrack)> {
        if self.tracks.len() >= MAX_REALTIME_TRACKS {
            return Err((EngineError::TrackLimit, track));
        }
        if self.track(track.id()).is_some() {
            let id = track.id().0;
            return Err((EngineError::DuplicateTrack(id), track));
        }
        let position = self.position_frame();
        let playing = self.is_playing();
        let PreparedTrack(mut inner) = track;
        inner.sequencer.seek(position);
        inner.sequencer.set_playing(playing);
        // `tracks` reserves MAX_REALTIME_TRACKS slots up front, so this push
        // never reallocates on the audio thread.
        self.tracks.push(inner);
        Ok(())
    }

    /// Replace a sample track's playback data at the current frame. Returns
    /// the previous playback so it can be released off the audio thread.
    pub fn replace_sample(
        &mut self,
        id: TrackId,
        playback: CompiledSamplePlayback,
    ) -> Result<CompiledSamplePlayback, (EngineError, CompiledSamplePlayback)> {
        let Some(track) = self.tracks.iter_mut().find(|track| track.id == id) else {
            return Err((EngineError::TrackNotFound(id.0), playback));
        };
        match &mut track.instrument {
            TrackInstrument::Sample(sampler) => Ok(sampler.replace_playback(playback)),
            TrackInstrument::Synth(_) => Err((EngineError::WrongTrackKind, playback)),
        }
    }

    pub fn is_playing(&self) -> bool {
        self.tracks
            .first()
            .is_some_and(|track| track.sequencer.is_playing())
    }

    /// Pause (false) freezes transport, scenes and lock state; resuming
    /// continues from the same frame.
    pub fn set_playing_all(&mut self, playing: bool) {
        for track in &mut self.tracks {
            if track.sequencer.is_playing() && !playing {
                track.instrument.handle(EngineCommand::Panic);
            }
            track.sequencer.set_playing(playing);
        }
    }

    pub fn toggle_playing_all(&mut self) -> bool {
        let playing = !self.is_playing();
        self.set_playing_all(playing);
        playing
    }

    /// Restart from frame 0: voices stop, locks restore to base, queued
    /// scenes are discarded and a running chain stops. The active scene's
    /// patterns stay selected.
    pub fn restart_all(&mut self) {
        for track in &mut self.tracks {
            track.instrument.handle(EngineCommand::Panic);
            track.set_locks(&[]);
            track.commit_pending_now();
            track.sequencer.restart();
        }
        self.pending_scene = None;
        self.chain_run = None;
    }

    /// Stop all voices and restore locked parameters to base. Transport,
    /// scenes and chains continue.
    pub fn panic_all(&mut self) {
        for track in &mut self.tracks {
            track.instrument.handle(EngineCommand::Panic);
            track.set_locks(&[]);
        }
        for bus in &mut self.send_buses {
            bus.rack.reset();
        }
        self.master_rack.reset();
        for track in &mut self.tracks {
            track.inserts.reset();
        }
    }

    fn track(&self, id: TrackId) -> Option<&RealtimeTrack> {
        self.tracks.iter().find(|track| track.id == id)
    }

    fn track_mut(&mut self, id: TrackId) -> Result<&mut RealtimeTrack, EngineError> {
        self.tracks
            .iter_mut()
            .find(|track| track.id == id)
            .ok_or(EngineError::TrackNotFound(id.0))
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
