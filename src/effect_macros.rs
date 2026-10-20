//! Spec 15 — performance macros for effect chains.
//!
//! Each effect chain (a track's inserts, a send bus or the master chain)
//! has two macro knobs. A macro points at one parameter of one effect in the
//! chain and drives it across the parameter's whole range using the
//! parameter's own response curve. A macro may also carry per-scene values,
//! which the engine applies when the scene launches, through the same
//! smoothed parameter path as a live edit (so recall never clicks and
//! respects active parameter locks).

use crate::effects::{effect_params, find_effect_param, EffectKind};
use crate::params::{EffectLocation, EffectParamId, EffectSlotId};
use crate::scenes::{Scene, SceneId};
use serde::{Deserialize, Serialize};

pub const MACROS_PER_CHAIN: u8 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneMacroValue {
    pub scene: SceneId,
    pub value: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EffectMacro {
    pub location: EffectLocation,
    /// Macro number within the chain: 1 or 2.
    pub index: u8,
    /// Effect slot within the chain.
    pub slot: u8,
    /// Parameter name (see the effect's descriptor table).
    pub param: String,
    /// Knob position, 0.0..=1.0.
    #[serde(default)]
    pub value: f32,
    /// Knob positions recalled when the named scene launches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scene_values: Vec<SceneMacroValue>,
}

fn unit(value: f32) -> bool {
    value.is_finite() && (0.0..=1.0).contains(&value)
}

impl EffectMacro {
    /// Maps a knob position onto the target parameter's range.
    pub fn parameter_value(&self, kind: EffectKind, position: f32) -> Option<(EffectParamId, f32)> {
        let id = find_effect_param(kind, &self.param)?;
        let descriptor = effect_params(kind)[usize::from(id.0)];
        Some((
            id,
            descriptor
                .curve
                .map(position, descriptor.min, descriptor.max),
        ))
    }

    pub fn slot_id(&self) -> EffectSlotId {
        EffectSlotId(self.slot)
    }

    /// Structural validation; `kind_at` resolves the effect kind of this
    /// macro's slot (None when the chain or slot does not exist).
    pub fn validate(
        &self,
        kind_at: &dyn Fn(EffectLocation, u8) -> Option<EffectKind>,
        scenes: &[Scene],
    ) -> Result<(), String> {
        if !(1..=MACROS_PER_CHAIN).contains(&self.index) {
            return Err(format!(
                "effect macro index must be 1 or {MACROS_PER_CHAIN}, got {}",
                self.index
            ));
        }
        let kind = kind_at(self.location, self.slot).ok_or_else(|| {
            format!(
                "effect macro {} targets {:?} slot {} which does not exist",
                self.index, self.location, self.slot
            )
        })?;
        if find_effect_param(kind, &self.param).is_none() {
            return Err(format!(
                "effect macro {} targets unknown {} parameter `{}`",
                self.index,
                kind.name(),
                self.param
            ));
        }
        if !unit(self.value) {
            return Err("effect macro value must be between 0.0 and 1.0".into());
        }
        for (position, entry) in self.scene_values.iter().enumerate() {
            if !unit(entry.value) {
                return Err("effect macro scene value must be between 0.0 and 1.0".into());
            }
            if !scenes.iter().any(|scene| scene.id == entry.scene) {
                return Err(format!(
                    "effect macro refers to unknown scene {}",
                    entry.scene.0
                ));
            }
            if self.scene_values[..position]
                .iter()
                .any(|earlier| earlier.scene == entry.scene)
            {
                return Err(format!(
                    "effect macro has two values for scene {}",
                    entry.scene.0
                ));
            }
        }
        Ok(())
    }
}

/// Validates the whole list: each macro, and at most one macro per
/// (chain, index).
pub fn validate_effect_macros(
    macros: &[EffectMacro],
    kind_at: &dyn Fn(EffectLocation, u8) -> Option<EffectKind>,
    scenes: &[Scene],
) -> Result<(), String> {
    for (position, entry) in macros.iter().enumerate() {
        entry.validate(kind_at, scenes)?;
        if macros[..position]
            .iter()
            .any(|earlier| earlier.location == entry.location && earlier.index == entry.index)
        {
            return Err(format!(
                "effect chain {:?} has two macros numbered {}",
                entry.location, entry.index
            ));
        }
    }
    Ok(())
}

/// Parameter writes for one scene: `(location, slot, param, value)`.
pub type SceneRecall = (EffectLocation, EffectSlotId, EffectParamId, f32);

/// Resolves the writes each scene applies, in macro order. Control-side.
pub fn compile_scene_recalls(
    macros: &[EffectMacro],
    scenes: &[Scene],
    kind_at: &dyn Fn(EffectLocation, u8) -> Option<EffectKind>,
) -> Vec<(SceneId, Vec<SceneRecall>)> {
    scenes
        .iter()
        .map(|scene| {
            let writes = macros
                .iter()
                .filter_map(|entry| {
                    let position = entry
                        .scene_values
                        .iter()
                        .find(|value| value.scene == scene.id)?
                        .value;
                    let kind = kind_at(entry.location, entry.slot)?;
                    let (param, value) = entry.parameter_value(kind, position)?;
                    Some((entry.location, entry.slot_id(), param, value))
                })
                .collect();
            (scene.id, writes)
        })
        .collect()
}
