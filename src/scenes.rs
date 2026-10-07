//! Spec 05 — scenes and scene chains (project-level, control side).
//!
//! Scenes are sparse overrides: a scene lists only the tracks it changes.
//! The engine compiles every scene and chain before audio starts, so a
//! launch at run time is a small index plus a frame.

use crate::TrackId;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const MAX_SCENES: usize = 64;
pub const MAX_CHAINS: usize = 16;
pub const MAX_CHAIN_STEPS: usize = 256;
pub const MAX_CHAIN_REPEATS: u16 = 999;
/// Chains count bars of this many beats.
pub const CHAIN_BEATS_PER_BAR: u32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SceneId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PatternId(pub u16);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ChainId(pub u16);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneTrackState {
    pub track_id: TrackId,
    pub pattern_id: PatternId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub muted: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gain: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scene {
    pub id: SceneId,
    pub name: String,
    pub track_states: Vec<SceneTrackState>,
}

/// Reserved for a later release; v1 projects must leave it unset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FollowAction {
    Next,
    RandomWeighted,
    Stop,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainStep {
    pub scene_id: SceneId,
    /// Number of bars (of [`CHAIN_BEATS_PER_BAR`] beats) the scene plays.
    pub repeats: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow: Option<FollowAction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneChain {
    pub id: ChainId,
    pub name: String,
    pub steps: Vec<ChainStep>,
    #[serde(default)]
    pub loop_chain: bool,
}

/// Resolves a scene or chain argument: a numeric ID first, then an exact
/// (case-insensitive) unique name.
pub fn resolve_scene(scenes: &[Scene], spec: &str) -> Result<SceneId, String> {
    if let Ok(id) = spec.parse::<u16>() {
        if scenes.iter().any(|scene| scene.id.0 == id) {
            return Ok(SceneId(id));
        }
    }
    let matches: Vec<_> = scenes
        .iter()
        .filter(|scene| scene.name.eq_ignore_ascii_case(spec))
        .collect();
    match matches.as_slice() {
        [scene] => Ok(scene.id),
        [] => Err(format!("unknown scene: {spec}")),
        _ => Err(format!(
            "scene name {spec} is ambiguous; use its numeric ID"
        )),
    }
}

pub fn resolve_chain(chains: &[SceneChain], spec: &str) -> Result<ChainId, String> {
    if let Ok(id) = spec.parse::<u16>() {
        if chains.iter().any(|chain| chain.id.0 == id) {
            return Ok(ChainId(id));
        }
    }
    let matches: Vec<_> = chains
        .iter()
        .filter(|chain| chain.name.eq_ignore_ascii_case(spec))
        .collect();
    match matches.as_slice() {
        [chain] => Ok(chain.id),
        [] => Err(format!("unknown chain: {spec}")),
        _ => Err(format!(
            "chain name {spec} is ambiguous; use its numeric ID"
        )),
    }
}

/// Validate scenes and chains against the project's tracks. `pattern_exists`
/// answers whether a track owns a pattern ID.
pub fn validate_scenes(
    scenes: &[Scene],
    chains: &[SceneChain],
    pattern_exists: impl Fn(TrackId, PatternId) -> bool,
) -> Result<(), String> {
    if scenes.len() > MAX_SCENES {
        return Err(format!("projects support at most {MAX_SCENES} scenes"));
    }
    let mut ids = HashSet::new();
    for scene in scenes {
        if !ids.insert(scene.id) {
            return Err(format!("duplicate scene id {}", scene.id.0));
        }
        if scene.name.trim().is_empty() {
            return Err(format!("scene {} needs a name", scene.id.0));
        }
        let mut tracks = HashSet::new();
        for state in &scene.track_states {
            if !tracks.insert(state.track_id) {
                return Err(format!(
                    "scene {} lists track {} twice",
                    scene.id.0, state.track_id.0
                ));
            }
            if !pattern_exists(state.track_id, state.pattern_id) {
                return Err(format!(
                    "scene {} references track {} pattern {}, which does not exist",
                    scene.id.0, state.track_id.0, state.pattern_id.0
                ));
            }
            if let Some(gain) = state.gain {
                if !gain.is_finite() || !(0.0..=2.0).contains(&gain) {
                    return Err(format!(
                        "scene {} track {} gain must be between 0.0 and 2.0",
                        scene.id.0, state.track_id.0
                    ));
                }
            }
        }
    }
    if chains.len() > MAX_CHAINS {
        return Err(format!("projects support at most {MAX_CHAINS} chains"));
    }
    let mut chain_ids = HashSet::new();
    for chain in chains {
        if !chain_ids.insert(chain.id) {
            return Err(format!("duplicate chain id {}", chain.id.0));
        }
        if chain.steps.is_empty() || chain.steps.len() > MAX_CHAIN_STEPS {
            return Err(format!(
                "chain {} must have between 1 and {MAX_CHAIN_STEPS} steps",
                chain.id.0
            ));
        }
        for step in &chain.steps {
            if !ids.contains(&step.scene_id) {
                return Err(format!(
                    "chain {} references unknown scene {}",
                    chain.id.0, step.scene_id.0
                ));
            }
            if !(1..=MAX_CHAIN_REPEATS).contains(&step.repeats) {
                return Err(format!(
                    "chain {} repeats must be between 1 and {MAX_CHAIN_REPEATS} bars",
                    chain.id.0
                ));
            }
            if step.follow.is_some() {
                return Err(format!(
                    "chain {} uses a follow action; follow actions are reserved and not supported yet",
                    chain.id.0
                ));
            }
        }
    }
    Ok(())
}

/// Frame at which bar `index` starts, using the same rounding as
/// [`crate::next_boundary_frame`].
pub fn bar_frame(index: u64, frames_per_bar: f64) -> u64 {
    let frame = index as f64 * frames_per_bar;
    if !frame.is_finite() || frame >= u64::MAX as f64 {
        u64::MAX
    } else {
        frame.round() as u64
    }
}

/// Index of the first bar starting at or after `frame`.
pub fn bar_at_or_after(frame: u64, frames_per_bar: f64) -> u64 {
    let mut index = (frame as f64 / frames_per_bar).floor() as u64;
    while bar_frame(index, frames_per_bar) < frame {
        index += 1;
    }
    index
}
