//! MIDI learn and persistent controller mapping (spec 06).
//!
//! This is control-thread code: it turns decoded [`MidiEvent`]s into typed
//! [`MappedOutput`]s that the session controller applies (and forwards to the
//! engine as bounded commands). Nothing here runs in the audio callback.
//!
//! Conversion rules:
//!
//! * CC values map to `value / 127` (exactly 0 and 1 at the endpoints).
//! * Pitch bend (`-8192..=8191`, see [`crate::decode_message`]) maps to 0..1
//!   piecewise so that -8192 -> 0, 0 -> 0.5 and 8191 -> 1 exactly.
//! * `inverted` flips the normalized value (`1 - v`) before range mapping.
//! * Linear: `min + (max - min) * v`; logarithmic: `min * (max / min)^v` when
//!   both endpoints are positive, otherwise linear. Results are clamped to the
//!   target descriptor's range and rounded for discrete descriptors.
//! * Button semantics apply to note sources, to any source mapped to an action,
//!   and to CC/pitch-bend sources whose [`ButtonMode`] is `Toggle` or
//!   `Trigger`. A CC in `Momentary` mode is treated as a continuous control
//!   (a momentary CC button sends 127/0 and therefore maps to max/min anyway).
//!   For CC, `value >= 64` is a press; for pitch bend, normalized `>= 0.75`.
//!
//! Scale (soft takeover) formula: with the previous hardware position `p`,
//! the new hardware position `h` and the current software position `t` (all
//! normalized, after inversion), moving up gives
//! `t' = t + (h - p) * (1 - t) / (1 - p)` and moving down gives
//! `t' = t - (p - h) * t / p`; i.e. the remaining hardware travel is mapped
//! onto the remaining software travel in the direction of motion. Reaching
//! an endpoint sets `t'` to that endpoint. Once `|h - t'| <= 1/127` the
//! mapping follows the hardware directly until pickup is invalidated. The
//! first message after (re)arming only records the hardware position.

use crate::params::{parse_synth_param, ParamCurve, ParamDescriptor};
use crate::{
    ActionId, EffectLocation, EffectParamId, EffectSlotId, GlobalParamId, MidiEvent,
    ParameterTarget, SampleParamId, SynthParamId, TrackId, TrackParamId,
};
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;

pub const DEFAULT_LEARN_TIMEOUT_MS: u64 = 15_000;
pub const MAX_MAPPINGS: usize = 256;

/// One CC step; also the pickup "equal" tolerance.
const PICKUP_TOLERANCE: f32 = 1.0 / 127.0 + 1e-6;
const PITCH_BEND_PRESS: f32 = 0.75;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct MappingId(pub u32);

/// Which MIDI input port a mapping listens to. `Any` is only ever chosen
/// explicitly by the user; learn always stores the exact port name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiPortMatch {
    Any,
    Exact { name: String },
}

impl MidiPortMatch {
    pub fn matches(&self, port_name: &str) -> bool {
        match self {
            Self::Any => true,
            Self::Exact { name } => name == port_name,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type")]
pub enum MidiControlMessage {
    ControlChange { controller: u8 },
    Note { note: u8 },
    PitchBend,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MidiSource {
    pub port: MidiPortMatch,
    /// Zero-based MIDI channel; `None` matches any channel.
    pub channel: Option<u8>,
    pub message: MidiControlMessage,
}

impl MidiSource {
    fn matches(&self, port_name: &str, channel: u8) -> bool {
        self.channel.is_none_or(|c| c == channel) && self.port.matches(port_name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickupMode {
    Jump,
    Pickup,
    Scale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonMode {
    Momentary,
    Toggle,
    Trigger,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingCurve {
    Linear,
    Logarithmic,
}

impl MappingCurve {
    fn param_curve(self) -> ParamCurve {
        match self {
            Self::Linear => ParamCurve::Linear,
            Self::Logarithmic => ParamCurve::Logarithmic,
        }
    }
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiMapping {
    pub id: MappingId,
    pub source: MidiSource,
    pub target: ParameterTarget,
    pub min: f32,
    pub max: f32,
    pub curve: MappingCurve,
    pub inverted: bool,
    pub pickup: PickupMode,
    pub button: ButtonMode,
    /// Orphaned mappings (target no longer exists) are kept but disabled.
    /// Missing in older files means enabled.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

impl MidiMapping {
    pub fn validate(&self) -> Result<(), String> {
        if let Some(channel) = self.source.channel {
            if channel > 15 {
                return Err(format!(
                    "mapping {}: MIDI channel must be 0..=15",
                    self.id.0
                ));
            }
        }
        match self.source.message {
            MidiControlMessage::ControlChange { controller } if controller > 127 => {
                return Err(format!("mapping {}: controller must be 0..=127", self.id.0));
            }
            MidiControlMessage::Note { note } if note > 127 => {
                return Err(format!("mapping {}: note must be 0..=127", self.id.0));
            }
            _ => {}
        }
        if let MidiPortMatch::Exact { name } = &self.source.port {
            if name.trim().is_empty() {
                return Err(format!(
                    "mapping {}: port name must not be empty",
                    self.id.0
                ));
            }
        }
        validate_range(self.min, self.max).map_err(|e| format!("mapping {}: {e}", self.id.0))
    }

    fn is_button(&self) -> bool {
        matches!(self.target, ParameterTarget::Action(_))
            || matches!(self.source.message, MidiControlMessage::Note { .. })
            || matches!(self.button, ButtonMode::Toggle | ButtonMode::Trigger)
    }
}

fn validate_range(min: f32, max: f32) -> Result<(), String> {
    if !min.is_finite() || !max.is_finite() {
        return Err("range endpoints must be finite".into());
    }
    if min == max {
        return Err("range min and max must differ".into());
    }
    Ok(())
}

/// What a mapped MIDI message asks the controller to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MappedOutput {
    SetParameter { target: ParameterTarget, value: f32 },
    Action(ActionId),
}

/// Resolves target ranges/current values; implemented by the session
/// controller (and by test fakes).
pub trait TargetResolver {
    fn descriptor(&self, target: ParameterTarget) -> Option<ParamDescriptor>;
    fn current_value(&self, target: ParameterTarget) -> Option<f32>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    /// Remove the existing mappings from the same source.
    Replace,
    /// Keep them; the source now drives several targets.
    AddSecondary,
    /// Abandon the learn without changes.
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LearnState {
    Idle,
    Awaiting {
        target: ParameterTarget,
        deadline_ms: u64,
        allow_notes: bool,
    },
    Captured {
        target: ParameterTarget,
        source: MidiSource,
        conflicts: Vec<MappingId>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LearnProgress {
    /// Not learning; mapped outputs (if any) were appended.
    NotLearning,
    /// Learning, and this message was neither captured nor applied.
    Ignored,
    /// The message was captured as the proposed source.
    Captured { conflicts: Vec<MappingId> },
}

/// Per-mapping control-thread runtime state.
#[derive(Debug, Clone, Copy, Default)]
struct MappingRuntime {
    /// Last normalized hardware position (after inversion).
    last_hw: Option<f32>,
    /// Pickup/Scale has caught the software value.
    picked: bool,
    /// Scale mode's own software position while converging.
    scale_value: Option<f32>,
    /// Button is currently held.
    pressed: bool,
    /// Known toggle state; `None` re-derives it from the resolver.
    toggle_on: Option<bool>,
}

impl MappingRuntime {
    fn rearm(&mut self) {
        self.picked = false;
        self.scale_value = None;
        self.toggle_on = None;
    }
}

/// Immutable lookup snapshot rebuilt on every mapping mutation: indices into
/// the id-sorted mapping list, grouped by message kind and number.
#[derive(Debug, Clone)]
struct CompiledLookup {
    cc: Vec<Vec<u16>>,
    note: Vec<Vec<u16>>,
    pitch_bend: Vec<u16>,
}

impl CompiledLookup {
    fn build(mappings: &[MidiMapping]) -> Self {
        let mut lookup = Self {
            cc: vec![Vec::new(); 128],
            note: vec![Vec::new(); 128],
            pitch_bend: Vec::new(),
        };
        for (index, mapping) in mappings.iter().enumerate() {
            let index = index as u16;
            match mapping.source.message {
                MidiControlMessage::ControlChange { controller } => {
                    lookup.cc[usize::from(controller & 0x7F)].push(index)
                }
                MidiControlMessage::Note { note } => {
                    lookup.note[usize::from(note & 0x7F)].push(index)
                }
                MidiControlMessage::PitchBend => lookup.pitch_bend.push(index),
            }
        }
        lookup
    }

    fn bucket(&self, event: &MidiEvent) -> &[u16] {
        match *event {
            MidiEvent::ControlChange { controller, .. } => &self.cc[usize::from(controller & 0x7F)],
            MidiEvent::NoteOn { note, .. } | MidiEvent::NoteOff { note, .. } => {
                &self.note[usize::from(note & 0x7F)]
            }
            MidiEvent::PitchBend { .. } => &self.pitch_bend,
        }
    }
}

/// Interpretation of one incoming message for a given mapping.
#[derive(Debug, Clone, Copy)]
struct Input {
    /// Normalized 0..1 hardware value (before inversion).
    normalized: f32,
    /// Button state implied by the message.
    pressed: bool,
}

fn event_channel(event: &MidiEvent) -> u8 {
    match *event {
        MidiEvent::NoteOn { channel, .. }
        | MidiEvent::NoteOff { channel, .. }
        | MidiEvent::ControlChange { channel, .. }
        | MidiEvent::PitchBend { channel, .. } => channel,
    }
}

/// Normalize a decoded pitch bend so that the endpoints and centre are exact.
pub fn normalize_pitch_bend(value: i16) -> f32 {
    let value = value.clamp(-8192, 8191);
    if value >= 0 {
        0.5 + 0.5 * f32::from(value) / 8191.0
    } else {
        0.5 + 0.5 * f32::from(value) / 8192.0
    }
}

fn interpret(event: &MidiEvent) -> Input {
    match *event {
        MidiEvent::NoteOn { velocity, .. } => Input {
            normalized: f32::from(velocity.min(127)) / 127.0,
            pressed: true,
        },
        MidiEvent::NoteOff { .. } => Input {
            normalized: 0.0,
            pressed: false,
        },
        MidiEvent::ControlChange { value, .. } => Input {
            normalized: f32::from(value.min(127)) / 127.0,
            pressed: value >= 64,
        },
        MidiEvent::PitchBend { value, .. } => {
            let normalized = normalize_pitch_bend(value);
            Input {
                normalized,
                pressed: normalized >= PITCH_BEND_PRESS,
            }
        }
    }
}

/// Map an already-inverted normalized value onto the mapping range, then
/// clamp/round according to the target descriptor.
fn value_from_norm(mapping: &MidiMapping, v: f32, descriptor: Option<&ParamDescriptor>) -> f32 {
    let mut value = if v >= 1.0 {
        mapping.max
    } else if v <= 0.0 {
        mapping.min
    } else {
        mapping.curve.param_curve().map(v, mapping.min, mapping.max)
    };
    if let Some(descriptor) = descriptor {
        if descriptor.curve == ParamCurve::Discrete {
            value = value.round();
        }
        let (lo, hi) = (
            descriptor.min.min(descriptor.max),
            descriptor.max.max(descriptor.min),
        );
        value = value.clamp(lo, hi);
    }
    value
}

/// Software value expressed in the mapping's (already inverted) normalized space.
fn software_norm(mapping: &MidiMapping, resolver: &dyn TargetResolver) -> Option<f32> {
    let current = resolver.current_value(mapping.target)?;
    if !current.is_finite() {
        return None;
    }
    Some(
        mapping
            .curve
            .param_curve()
            .normalize(current, mapping.min, mapping.max),
    )
}

fn apply_inversion(mapping: &MidiMapping, normalized: f32) -> f32 {
    if mapping.inverted {
        1.0 - normalized
    } else {
        normalized
    }
}

fn process_button(
    mapping: &MidiMapping,
    rt: &mut MappingRuntime,
    pressed: bool,
    resolver: &dyn TargetResolver,
) -> Option<MappedOutput> {
    let press = pressed && !rt.pressed;
    let release = !pressed && rt.pressed;
    rt.pressed = pressed;
    if !press && !release {
        return None;
    }
    if let ParameterTarget::Action(action) = mapping.target {
        return press.then_some(MappedOutput::Action(action));
    }
    let descriptor = resolver.descriptor(mapping.target);
    let on = value_from_norm(mapping, apply_inversion(mapping, 1.0), descriptor.as_ref());
    let off = value_from_norm(mapping, apply_inversion(mapping, 0.0), descriptor.as_ref());
    let value = match (mapping.button, press) {
        (ButtonMode::Momentary, true) | (ButtonMode::Trigger, true) => on,
        (ButtonMode::Momentary, false) => off,
        (ButtonMode::Toggle, true) => {
            let currently_on = rt.toggle_on.unwrap_or_else(|| {
                resolver
                    .current_value(mapping.target)
                    .filter(|c| c.is_finite())
                    .is_some_and(|c| (c - on).abs() < (c - off).abs())
            });
            rt.toggle_on = Some(!currently_on);
            if currently_on {
                off
            } else {
                on
            }
        }
        (ButtonMode::Toggle, false) | (ButtonMode::Trigger, false) => return None,
    };
    Some(MappedOutput::SetParameter {
        target: mapping.target,
        value,
    })
}

fn process_continuous(
    mapping: &MidiMapping,
    rt: &mut MappingRuntime,
    normalized: f32,
    resolver: &dyn TargetResolver,
) -> Option<MappedOutput> {
    let v = apply_inversion(mapping, normalized);
    let last = rt.last_hw.replace(v);
    let emit_norm = match mapping.pickup {
        PickupMode::Jump => Some(v),
        PickupMode::Pickup => {
            if !rt.picked {
                rt.picked = match software_norm(mapping, resolver) {
                    None => true,
                    Some(t) => {
                        (v - t).abs() <= PICKUP_TOLERANCE
                            || last.is_some_and(|p| (p - t) * (v - t) <= 0.0)
                    }
                };
            }
            rt.picked.then_some(v)
        }
        PickupMode::Scale => {
            if rt.picked {
                Some(v)
            } else {
                match rt.scale_value.or_else(|| software_norm(mapping, resolver)) {
                    None => {
                        rt.picked = true;
                        Some(v)
                    }
                    Some(t) if (v - t).abs() <= PICKUP_TOLERANCE => {
                        rt.picked = true;
                        rt.scale_value = None;
                        Some(v)
                    }
                    Some(t) => match last {
                        None => None,
                        Some(p) if v == p => None,
                        Some(p) => {
                            let next = if v >= 1.0 {
                                1.0
                            } else if v <= 0.0 {
                                0.0
                            } else if v > p {
                                t + (v - p) * (1.0 - t) / (1.0 - p)
                            } else {
                                t - (p - v) * t / p
                            }
                            .clamp(0.0, 1.0);
                            if (v - next).abs() <= PICKUP_TOLERANCE {
                                rt.picked = true;
                                rt.scale_value = None;
                            } else {
                                rt.scale_value = Some(next);
                            }
                            Some(next)
                        }
                    },
                }
            }
        }
    }?;
    let descriptor = resolver.descriptor(mapping.target);
    Some(MappedOutput::SetParameter {
        target: mapping.target,
        value: value_from_norm(mapping, emit_norm, descriptor.as_ref()),
    })
}

/// MIDI learn state machine plus the active mapping table.
#[derive(Debug, Clone)]
pub struct MidiLearn {
    /// Sorted by id; this order is the deterministic lookup order.
    mappings: Vec<MidiMapping>,
    /// Runtime state keyed by mapping id, parallel to `mappings`.
    runtime: Vec<(MappingId, MappingRuntime)>,
    compiled: CompiledLookup,
    state: LearnState,
    next_id: u32,
}

impl MidiLearn {
    /// Validates every mapping, requires unique ids and at most
    /// [`MAX_MAPPINGS`] entries.
    pub fn new(mut mappings: Vec<MidiMapping>) -> Result<Self, String> {
        if mappings.len() > MAX_MAPPINGS {
            return Err(format!(
                "at most {MAX_MAPPINGS} MIDI mappings are supported"
            ));
        }
        for mapping in &mappings {
            mapping.validate()?;
        }
        mappings.sort_by_key(|m| m.id);
        if let Some(pair) = mappings.windows(2).find(|w| w[0].id == w[1].id) {
            return Err(format!("duplicate MIDI mapping id {}", pair[0].id.0));
        }
        let next_id = mappings
            .last()
            .map_or(1, |m| m.id.0.saturating_add(1))
            .max(1);
        let mut learn = Self {
            runtime: Vec::new(),
            compiled: CompiledLookup::build(&[]),
            mappings,
            state: LearnState::Idle,
            next_id,
        };
        learn.recompile();
        Ok(learn)
    }

    pub fn mappings(&self) -> &[MidiMapping] {
        &self.mappings
    }

    pub fn learn_state(&self) -> &LearnState {
        &self.state
    }

    pub fn begin_learn(&mut self, target: ParameterTarget, now_ms: u64, allow_notes: bool) {
        self.state = LearnState::Awaiting {
            target,
            deadline_ms: now_ms.saturating_add(DEFAULT_LEARN_TIMEOUT_MS),
            allow_notes,
        };
    }

    pub fn cancel_learn(&mut self) {
        self.state = LearnState::Idle;
    }

    /// Ids of mappings that would respond to this message, in id order.
    pub fn matching_ids(&self, port_name: &str, event: MidiEvent) -> Vec<MappingId> {
        let channel = event_channel(&event);
        self.compiled
            .bucket(&event)
            .iter()
            .map(|&i| &self.mappings[usize::from(i)])
            .filter(|m| m.enabled && m.source.matches(port_name, channel))
            .map(|m| m.id)
            .collect()
    }

    /// Process one incoming message. While awaiting a source, an eligible
    /// CC/pitch bend (or note-on when notes are allowed) is captured instead
    /// of applied and nothing is emitted. While a capture awaits
    /// confirmation, further messages from the captured control are
    /// swallowed; everything else is processed normally. Otherwise at most
    /// one output per matching mapping is appended to `out`.
    pub fn handle(
        &mut self,
        port_name: &str,
        event: MidiEvent,
        now_ms: u64,
        resolver: &dyn TargetResolver,
        out: &mut Vec<MappedOutput>,
    ) -> LearnProgress {
        let channel = event_channel(&event);
        match &self.state {
            LearnState::Awaiting { deadline_ms, .. } if now_ms >= *deadline_ms => {
                self.state = LearnState::Idle;
            }
            LearnState::Awaiting {
                target,
                allow_notes,
                ..
            } => {
                let message = match event {
                    MidiEvent::ControlChange { controller, .. } => {
                        MidiControlMessage::ControlChange { controller }
                    }
                    MidiEvent::PitchBend { .. } => MidiControlMessage::PitchBend,
                    MidiEvent::NoteOn { note, .. } if *allow_notes => {
                        MidiControlMessage::Note { note }
                    }
                    MidiEvent::NoteOn { .. } | MidiEvent::NoteOff { .. } => {
                        return LearnProgress::Ignored
                    }
                };
                let target = *target;
                let source = MidiSource {
                    port: MidiPortMatch::Exact {
                        name: port_name.to_owned(),
                    },
                    channel: Some(channel),
                    message,
                };
                let conflicts: Vec<MappingId> = self
                    .mappings
                    .iter()
                    .filter(|m| m.source.message == message && m.source.matches(port_name, channel))
                    .map(|m| m.id)
                    .collect();
                self.state = LearnState::Captured {
                    target,
                    source,
                    conflicts: conflicts.clone(),
                };
                return LearnProgress::Captured { conflicts };
            }
            LearnState::Captured { source, .. } => {
                let same_control = match (source.message, event) {
                    (
                        MidiControlMessage::ControlChange { controller: a },
                        MidiEvent::ControlChange { controller: b, .. },
                    ) => a == b,
                    (MidiControlMessage::PitchBend, MidiEvent::PitchBend { .. }) => true,
                    (
                        MidiControlMessage::Note { note: a },
                        MidiEvent::NoteOn { note: b, .. } | MidiEvent::NoteOff { note: b, .. },
                    ) => a == b,
                    _ => false,
                };
                if same_control && source.matches(port_name, channel) {
                    return LearnProgress::Ignored;
                }
            }
            LearnState::Idle => {}
        }

        let input = interpret(&event);
        let Self {
            mappings,
            runtime,
            compiled,
            ..
        } = self;
        for &index in compiled.bucket(&event) {
            let index = usize::from(index);
            let mapping = &mappings[index];
            if !mapping.enabled || !mapping.source.matches(port_name, channel) {
                continue;
            }
            let rt = &mut runtime[index].1;
            let output = if mapping.is_button() {
                process_button(mapping, rt, input.pressed, resolver)
            } else {
                process_continuous(mapping, rt, input.normalized, resolver)
            };
            if let Some(output) = output {
                if let MappedOutput::SetParameter { target, .. } = output {
                    // Another control moved this target: re-arm the others.
                    for (other, other_rt) in mappings.iter().zip(runtime.iter_mut()) {
                        if other.target == target && other.id != mapping.id {
                            other_rt.1.rearm();
                        }
                    }
                }
                out.push(output);
            }
        }
        LearnProgress::NotLearning
    }

    /// Returns true when a pending learn timed out (state -> Idle).
    pub fn tick(&mut self, now_ms: u64) -> bool {
        match self.state {
            LearnState::Awaiting { deadline_ms, .. } if now_ms >= deadline_ms => {
                self.state = LearnState::Idle;
                true
            }
            _ => false,
        }
    }

    /// Confirm a captured learn. CC/pitch bend default to Pickup +
    /// Momentary, notes to Jump + Toggle; action targets use Jump + Trigger.
    /// The range and curve come from the target descriptor.
    pub fn confirm(
        &mut self,
        policy: ConflictPolicy,
        resolver: &dyn TargetResolver,
    ) -> Result<Option<MappingId>, String> {
        let LearnState::Captured {
            target,
            source,
            conflicts,
        } = &self.state
        else {
            return Err("no captured MIDI source to confirm".into());
        };
        if policy == ConflictPolicy::Cancel {
            self.state = LearnState::Idle;
            return Ok(None);
        }
        let (target, source) = (*target, source.clone());
        let removed = if policy == ConflictPolicy::Replace {
            conflicts.len()
        } else {
            0
        };
        if self.mappings.len() - removed >= MAX_MAPPINGS {
            return Err(format!(
                "at most {MAX_MAPPINGS} MIDI mappings are supported"
            ));
        }
        let is_action = matches!(target, ParameterTarget::Action(_));
        let (min, max, curve) = if is_action {
            (0.0, 1.0, MappingCurve::Linear)
        } else {
            let descriptor = resolver
                .descriptor(target)
                .ok_or_else(|| format!("target {} does not exist", format_target(target)))?;
            let curve = if descriptor.curve == ParamCurve::Logarithmic {
                MappingCurve::Logarithmic
            } else {
                MappingCurve::Linear
            };
            (descriptor.min, descriptor.max, curve)
        };
        let is_note = matches!(source.message, MidiControlMessage::Note { .. });
        let (pickup, button) = match (is_action, is_note) {
            (true, _) => (PickupMode::Jump, ButtonMode::Trigger),
            (false, true) => (PickupMode::Jump, ButtonMode::Toggle),
            (false, false) => (PickupMode::Pickup, ButtonMode::Momentary),
        };
        let id = MappingId(self.next_id);
        let mapping = MidiMapping {
            id,
            source,
            target,
            min,
            max,
            curve,
            inverted: false,
            pickup,
            button,
            enabled: true,
        };
        mapping.validate()?;
        if policy == ConflictPolicy::Replace {
            let conflicts = conflicts.clone();
            self.mappings.retain(|m| !conflicts.contains(&m.id));
        }
        self.next_id = self.next_id.saturating_add(1);
        self.mappings.push(mapping);
        self.state = LearnState::Idle;
        self.recompile();
        Ok(Some(id))
    }

    pub fn remove(&mut self, id: MappingId) -> bool {
        let before = self.mappings.len();
        self.mappings.retain(|m| m.id != id);
        let removed = self.mappings.len() != before;
        if removed {
            self.recompile();
        }
        removed
    }

    pub fn set_range(&mut self, id: MappingId, min: f32, max: f32) -> Result<(), String> {
        validate_range(min, max)?;
        self.edit(id, |m| {
            m.min = min;
            m.max = max;
        })
    }

    pub fn set_inverted(&mut self, id: MappingId, inverted: bool) -> Result<(), String> {
        self.edit(id, |m| m.inverted = inverted)
    }

    pub fn set_pickup(&mut self, id: MappingId, pickup: PickupMode) -> Result<(), String> {
        self.edit(id, |m| m.pickup = pickup)
    }

    pub fn set_button(&mut self, id: MappingId, button: ButtonMode) -> Result<(), String> {
        self.edit(id, |m| m.button = button)
    }

    pub fn set_enabled(&mut self, id: MappingId, enabled: bool) -> Result<(), String> {
        self.edit(id, |m| m.enabled = enabled)
    }

    fn edit(&mut self, id: MappingId, change: impl FnOnce(&mut MidiMapping)) -> Result<(), String> {
        let index = self
            .mappings
            .iter()
            .position(|m| m.id == id)
            .ok_or_else(|| format!("no MIDI mapping {}", id.0))?;
        change(&mut self.mappings[index]);
        self.runtime[index].1 = MappingRuntime::default();
        Ok(())
    }

    /// Disable (never retarget) mappings whose target no longer exists;
    /// returns the ids of all orphaned mappings in id order.
    pub fn disable_orphans(&mut self, exists: impl Fn(ParameterTarget) -> bool) -> Vec<MappingId> {
        let mut orphans = Vec::new();
        for (mapping, rt) in self.mappings.iter_mut().zip(self.runtime.iter_mut()) {
            if !exists(mapping.target) {
                mapping.enabled = false;
                rt.1 = MappingRuntime::default();
                orphans.push(mapping.id);
            }
        }
        orphans
    }

    /// Call when a target value changes from elsewhere (UI, lock restore)
    /// so pickup/scale/toggle state re-arms.
    pub fn invalidate_pickup(&mut self, target: ParameterTarget) {
        for (mapping, rt) in self.mappings.iter().zip(self.runtime.iter_mut()) {
            if mapping.target == target {
                rt.1.rearm();
            }
        }
    }

    /// One-line human summary for the `mappings` command.
    pub fn describe(&self, id: MappingId) -> Option<String> {
        let m = self.mappings.iter().find(|m| m.id == id)?;
        let mut line = format!("#{} ", m.id.0);
        match m.source.message {
            MidiControlMessage::ControlChange { controller } => {
                let _ = write!(line, "cc {controller}");
            }
            MidiControlMessage::Note { note } => {
                let _ = write!(line, "note {note}");
            }
            MidiControlMessage::PitchBend => line.push_str("pitch bend"),
        }
        match m.source.channel {
            Some(channel) => {
                let _ = write!(line, " ch {}", u16::from(channel) + 1);
            }
            None => line.push_str(" any ch"),
        }
        match &m.source.port {
            MidiPortMatch::Any => line.push_str(" any port"),
            MidiPortMatch::Exact { name } => {
                let _ = write!(line, " on {name:?}");
            }
        }
        let _ = write!(line, " -> {}", format_target(m.target));
        if !matches!(m.target, ParameterTarget::Action(_)) {
            let curve = match m.curve {
                MappingCurve::Linear => "lin",
                MappingCurve::Logarithmic => "log",
            };
            let _ = write!(line, " [{}..{} {curve}", m.min, m.max);
            if m.inverted {
                line.push_str(" inverted");
            }
            let pickup = match m.pickup {
                PickupMode::Jump => "jump",
                PickupMode::Pickup => "pickup",
                PickupMode::Scale => "scale",
            };
            let button = match m.button {
                ButtonMode::Momentary => "momentary",
                ButtonMode::Toggle => "toggle",
                ButtonMode::Trigger => "trigger",
            };
            let _ = write!(line, ", {pickup}, {button}]");
        }
        if !m.enabled {
            line.push_str(" (disabled)");
        }
        Some(line)
    }

    fn recompile(&mut self) {
        self.mappings.sort_by_key(|m| m.id);
        let old = std::mem::take(&mut self.runtime);
        self.runtime = self
            .mappings
            .iter()
            .map(|m| {
                let state = old
                    .iter()
                    .find(|(id, _)| *id == m.id)
                    .map(|(_, rt)| *rt)
                    .unwrap_or_default();
                (m.id, state)
            })
            .collect();
        self.compiled = CompiledLookup::build(&self.mappings);
    }
}

/// Typed form of the MIDI learn text commands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LearnCommand {
    Learn {
        target: ParameterTarget,
        allow_notes: bool,
    },
    Cancel,
    /// `None` means no policy was given: the controller must ask for one when
    /// the capture has conflicts.
    Confirm {
        policy: Option<ConflictPolicy>,
    },
    Unlearn(MappingId),
    List,
    SetRange {
        id: MappingId,
        min: f32,
        max: f32,
    },
    SetInverted {
        id: MappingId,
        inverted: bool,
    },
    SetPickup {
        id: MappingId,
        mode: PickupMode,
    },
    SetButton {
        id: MappingId,
        mode: ButtonMode,
    },
}

fn parse_mapping_id(token: &str) -> Result<MappingId, String> {
    token
        .strip_prefix('#')
        .unwrap_or(token)
        .parse::<u32>()
        .map(MappingId)
        .map_err(|_| format!("invalid mapping id {token:?}"))
}

fn parse_finite(token: &str) -> Result<f32, String> {
    token
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("invalid number {token:?}"))
}

pub fn parse_learn_command(line: &str, selected_track: TrackId) -> Result<LearnCommand, String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    match tokens.as_slice() {
        ["learn", "cancel"] => Ok(LearnCommand::Cancel),
        ["learn", "confirm"] => Ok(LearnCommand::Confirm { policy: None }),
        ["learn", "confirm", policy] => {
            let policy = match *policy {
                "replace" => ConflictPolicy::Replace,
                "add" | "add_secondary" => ConflictPolicy::AddSecondary,
                "cancel" => ConflictPolicy::Cancel,
                other => return Err(format!("unknown conflict policy {other:?}")),
            };
            Ok(LearnCommand::Confirm {
                policy: Some(policy),
            })
        }
        ["learn", "notes", rest @ ..] if !rest.is_empty() => Ok(LearnCommand::Learn {
            target: parse_target(&rest.join(" "), selected_track)?,
            allow_notes: true,
        }),
        ["learn", rest @ ..] if !rest.is_empty() => Ok(LearnCommand::Learn {
            target: parse_target(&rest.join(" "), selected_track)?,
            allow_notes: false,
        }),
        ["unlearn", id] => Ok(LearnCommand::Unlearn(parse_mapping_id(id)?)),
        ["mappings"] => Ok(LearnCommand::List),
        ["mapping", id, "range", min, max] => Ok(LearnCommand::SetRange {
            id: parse_mapping_id(id)?,
            min: parse_finite(min)?,
            max: parse_finite(max)?,
        }),
        ["mapping", id, "invert", state] => {
            let inverted = match *state {
                "on" => true,
                "off" => false,
                other => return Err(format!("expected on|off, got {other:?}")),
            };
            Ok(LearnCommand::SetInverted {
                id: parse_mapping_id(id)?,
                inverted,
            })
        }
        ["mapping", id, "pickup", mode] => {
            let mode = match *mode {
                "jump" => PickupMode::Jump,
                "pickup" => PickupMode::Pickup,
                "scale" => PickupMode::Scale,
                other => return Err(format!("expected jump|pickup|scale, got {other:?}")),
            };
            Ok(LearnCommand::SetPickup {
                id: parse_mapping_id(id)?,
                mode,
            })
        }
        ["mapping", id, "button", mode] => {
            let mode = match *mode {
                "momentary" => ButtonMode::Momentary,
                "toggle" => ButtonMode::Toggle,
                "trigger" => ButtonMode::Trigger,
                other => return Err(format!("expected momentary|toggle|trigger, got {other:?}")),
            };
            Ok(LearnCommand::SetButton {
                id: parse_mapping_id(id)?,
                mode,
            })
        }
        _ => Err(format!("unrecognized MIDI learn command {line:?}")),
    }
}

/// Parse a target name. See the module docs of the commands for the syntax;
/// `track <id> <name>` addresses an explicit track, otherwise track-scoped
/// names use `selected_track`.
pub fn parse_target(spec: &str, selected_track: TrackId) -> Result<ParameterTarget, String> {
    let tokens: Vec<&str> = spec.split_whitespace().collect();
    match tokens.as_slice() {
        [name] => parse_scoped_target(name, selected_track),
        ["track", id, name] => {
            let track = id
                .parse::<u16>()
                .map(TrackId)
                .map_err(|_| format!("invalid track id {id:?}"))?;
            parse_scoped_target(name, track)
        }
        _ => Err(format!("invalid target {spec:?}")),
    }
}

fn parse_scoped_target(name: &str, track: TrackId) -> Result<ParameterTarget, String> {
    let unknown = || format!("unknown target {name:?}");
    let action = |action| Ok(ParameterTarget::Action(action));
    match name {
        "master.gain" => return Ok(ParameterTarget::Global(GlobalParamId::MasterGain)),
        "transport.play" => return action(ActionId::TogglePlay),
        "transport.restart" => return action(ActionId::Restart),
        "panic" => return action(ActionId::Panic),
        "scene.next" => return action(ActionId::SceneNext),
        "scene.prev" => return action(ActionId::ScenePrev),
        _ => {}
    }
    if let Some(id) = name.strip_prefix("scene.launch.") {
        let id = id
            .parse::<u16>()
            .map_err(|_| format!("invalid scene id {id:?}"))?;
        return action(ActionId::SceneLaunch(id));
    }
    let track_param = |param| Ok(ParameterTarget::Track { track, param });
    match name.strip_prefix("track.").unwrap_or(name) {
        "gain" => return track_param(TrackParamId::Gain),
        "pan" => return track_param(TrackParamId::Pan),
        "send_a" => return track_param(TrackParamId::SendA),
        "send_b" => return track_param(TrackParamId::SendB),
        "mute" => return track_param(TrackParamId::Mute),
        "solo" => return track_param(TrackParamId::Solo),
        _ => {}
    }
    if let Some(param) = name.strip_prefix("sample.") {
        let param = match param {
            "pitch" => SampleParamId::Pitch,
            "gain" => SampleParamId::Gain,
            "reverse" => SampleParamId::Reverse,
            _ => return Err(unknown()),
        };
        return Ok(ParameterTarget::Sample { track, param });
    }
    if let Some(rest) = name.strip_prefix("fx.") {
        let parts: Vec<&str> = rest.split('.').collect();
        let [location, slot, param] = parts.as_slice() else {
            return Err(format!(
                "effect targets look like fx.<location>.<slot>.<param>, got {name:?}"
            ));
        };
        let location = match *location {
            "track" => EffectLocation::Track(track),
            "send_a" => EffectLocation::Send(0),
            "send_b" => EffectLocation::Send(1),
            "master" => EffectLocation::Master,
            other => return Err(format!("unknown effect location {other:?}")),
        };
        let slot = slot
            .parse::<u8>()
            .map(EffectSlotId)
            .map_err(|_| format!("invalid effect slot {slot:?}"))?;
        let param = param
            .parse::<u8>()
            .map(EffectParamId)
            .map_err(|_| format!("invalid effect parameter {param:?}"))?;
        return Ok(ParameterTarget::Effect {
            location,
            slot,
            param,
        });
    }
    let synth_name = name
        .strip_prefix("synth.")
        .unwrap_or(name)
        .replace('.', "_");
    match parse_synth_param(&synth_name) {
        Some(SynthParamId::Oscillator | SynthParamId::FilterMode) => Err(format!(
            "{name:?} is a structural choice and cannot be MIDI-mapped"
        )),
        Some(param) => Ok(ParameterTarget::Synth { track, param }),
        None => Err(unknown()),
    }
}

fn synth_param_name(param: SynthParamId) -> String {
    match param {
        SynthParamId::FilterCutoff => "filter.cutoff".into(),
        SynthParamId::FilterResonance => "filter.resonance".into(),
        other => serde_json::to_value(other)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
    }
}

/// Inverse of [`parse_target`], always using the explicit `track <id>` form.
pub fn format_target(target: ParameterTarget) -> String {
    match target {
        ParameterTarget::Global(GlobalParamId::MasterGain) => "master.gain".into(),
        ParameterTarget::Track { track, param } => {
            let name = match param {
                TrackParamId::Gain => "gain",
                TrackParamId::Pan => "pan",
                TrackParamId::SendA => "send_a",
                TrackParamId::SendB => "send_b",
                TrackParamId::Mute => "mute",
                TrackParamId::Solo => "solo",
            };
            format!("track {} {name}", track.0)
        }
        ParameterTarget::Synth { track, param } => {
            format!("track {} {}", track.0, synth_param_name(param))
        }
        ParameterTarget::Sample { track, param } => {
            let name = match param {
                SampleParamId::Pitch => "pitch",
                SampleParamId::Gain => "gain",
                SampleParamId::Reverse => "reverse",
            };
            format!("track {} sample.{name}", track.0)
        }
        ParameterTarget::Effect {
            location,
            slot,
            param,
        } => {
            let suffix = format!("{}.{}", slot.0, param.0);
            match location {
                EffectLocation::Track(track) => format!("track {} fx.track.{suffix}", track.0),
                EffectLocation::Send(0) => format!("fx.send_a.{suffix}"),
                EffectLocation::Send(1) => format!("fx.send_b.{suffix}"),
                EffectLocation::Send(bus) => format!("fx.send_{bus}.{suffix}"),
                EffectLocation::Master => format!("fx.master.{suffix}"),
            }
        }
        ParameterTarget::Action(action) => match action {
            ActionId::TogglePlay => "transport.play".into(),
            ActionId::Restart => "transport.restart".into(),
            ActionId::Panic => "panic".into(),
            ActionId::SceneNext => "scene.next".into(),
            ActionId::ScenePrev => "scene.prev".into(),
            ActionId::SceneLaunch(id) => format!("scene.launch.{id}"),
        },
    }
}
