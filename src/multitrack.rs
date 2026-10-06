use crate::{ChannelStrip, EngineCommand, LiveSequencer, Oscillator, Pattern, RealtimeSynth};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_REALTIME_TRACKS: usize = 16;

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
    strip: ChannelStrip,
    soloed: bool,
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
        let strip = ChannelStrip {
            gain: definition.gain,
            pan: definition.pan,
            muted: definition.muted,
        };

        Ok(Self {
            id: definition.id,
            sequencer,
            synth,
            strip,
            soloed: definition.soloed,
        })
    }

    fn is_audible(&self, any_soloed: bool) -> bool {
        !self.strip.muted && (!any_soloed || self.soloed)
    }

    fn next_stereo_frame(
        &mut self,
        any_soloed: bool,
        commands: &mut Vec<EngineCommand>,
    ) -> (f32, f32) {
        self.sequencer.fill_commands(commands);
        for command in commands.drain(..) {
            self.synth.handle(command);
        }

        let mono = self.synth.next_sample();
        if !self.is_audible(any_soloed) {
            return (0.0, 0.0);
        }
        self.strip.process_mono(mono)
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
        if definitions.is_empty() {
            return Err("multi-track engine requires at least one track".into());
        }
        if definitions.len() > MAX_REALTIME_TRACKS {
            return Err(format!(
                "multi-track engine supports at most {MAX_REALTIME_TRACKS} tracks"
            ));
        }

        let mut ids = HashSet::with_capacity(definitions.len());
        for definition in &definitions {
            definition.validate()?;
            if !ids.insert(definition.id) {
                return Err(format!("duplicate track id: {}", definition.id.0));
            }
        }

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

    pub fn track_position(&self, id: TrackId) -> Option<u64> {
        self.track(id).map(|track| track.sequencer.position_frame())
    }

    pub fn track_is_audible(&self, id: TrackId) -> bool {
        let any_soloed = self.tracks.iter().any(|track| track.soloed);
        self.track(id)
            .is_some_and(|track| track.is_audible(any_soloed))
    }

    pub fn set_gain(&mut self, id: TrackId, gain: f32) -> Result<(), String> {
        if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
            return Err("track gain must be finite and between 0.0 and 2.0".into());
        }
        self.track_mut(id)?.strip.gain = gain;
        Ok(())
    }

    pub fn set_pan(&mut self, id: TrackId, pan: f32) -> Result<(), String> {
        if !pan.is_finite() || !(-1.0..=1.0).contains(&pan) {
            return Err("track pan must be finite and between -1.0 and 1.0".into());
        }
        self.track_mut(id)?.strip.pan = pan;
        Ok(())
    }

    pub fn set_mute(&mut self, id: TrackId, muted: bool) -> Result<(), String> {
        self.track_mut(id)?.strip.muted = muted;
        Ok(())
    }

    pub fn set_solo(&mut self, id: TrackId, soloed: bool) -> Result<(), String> {
        self.track_mut(id)?.soloed = soloed;
        Ok(())
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
