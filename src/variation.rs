//! Spec 12 — invariant-locked pattern variation (control-thread only).
//!
//! Everything in this module runs on the control thread: profile parsing and
//! validation, invariant evaluation, the seeded candidate generator, proposal
//! lifecycle and textual diff formatting. Nothing here is called from the
//! audio callback.
//!
//! # Determinism
//!
//! A proposal is a pure function of the source pattern (including its
//! invariant profile), the [`VariationRequest`] (seed and amount) and
//! [`VARIATION_ALGORITHM_VERSION`]. The generator uses a self-contained
//! SplitMix64 PRNG, iterates only over vectors (never hash maps), evaluates
//! rules in sorted rule-ID order and only uses IEEE-754 basic arithmetic, so
//! the same input yields the same proposal on every supported platform.
//! Pattern hashes are FNV-1a 64 over an explicit little-endian encoding (see
//! [`pattern_hash`]).

use crate::editor::PatternEditor;
use crate::pattern::{Pattern, PatternStep, MAX_PATTERN_STEPS};
use serde::{Deserialize, Serialize};
use std::fmt;

pub const INVARIANT_SCHEMA_VERSION: u32 = 1;
/// Version of the mutation algorithm, independent of the invariant schema.
pub const VARIATION_ALGORITHM_VERSION: u32 = 1;
/// Maximum candidates tried by one [`generate_variation`] call.
pub const MAX_VARIATION_ATTEMPTS: u32 = 512;
/// Default `--amount` for `variation preview`.
pub const DEFAULT_VARIATION_AMOUNT: f32 = 0.25;

/// Stable rule IDs used by `variation lock ...` commands.
pub const ANCHORS_RULE_ID: &str = "anchors";
pub const RHYTHM_RULE_ID: &str = "rhythm";
pub const CONTOUR_RULE_ID: &str = "contour";
pub const COUNT_RULE_ID: &str = "count";
pub const NOTE_RANGE_RULE_ID: &str = "note_range";

const MAX_RULE_ID_LEN: usize = 64;
const MAX_RATCHETS: u8 = 8;
const MAX_MICROTIMING_FRAMES: i32 = 192_000;
const MAX_SWING: f32 = 0.75;
/// Largest semitone move applied by one note operation.
const MAX_NOTE_MOVE: i16 = 12;
/// Velocity/gate/probability move by `k * UNIT_STEP`, `k` in 1..=4.
const UNIT_STEP: f32 = 0.05;
/// Microtiming moves by `k * MICROTIMING_STEP_FRAMES`, `k` in 1..=10.
const MICROTIMING_STEP_FRAMES: i32 = 24;
/// Swing moves by `k * SWING_STEP`, `k` in 1..=5.
const SWING_STEP: f32 = 0.02;
const DEFAULT_NEW_NOTE: u8 = 60;
const DEFAULT_NEW_VELOCITY: f32 = 0.8;
const DEFAULT_NEW_GATE: f32 = 0.5;

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

/// Optional invariant profile persisted inside Pattern JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvariantProfile {
    pub version: u32,
    #[serde(default)]
    pub rules: Vec<InvariantRule>,
}

/// One invariant rule. Persisted as a flat object:
/// `{"id": "...", "kind": "...", "enabled": true, ...params}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "RawInvariantRule", into = "RawInvariantRule")]
pub struct InvariantRule {
    pub id: String,
    pub enabled: bool,
    pub kind: InvariantKind,
}

/// Invariant kinds. Step positions in `anchor_steps` are one-based.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InvariantKind {
    AnchorSteps { steps: Vec<u16> },
    ActivityMask,
    PitchContour,
    EventCount,
    NoteRange { low: u8, high: u8 },
}

impl InvariantKind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::AnchorSteps { .. } => "anchor_steps",
            Self::ActivityMask => "activity_mask",
            Self::PitchContour => "pitch_contour",
            Self::EventCount => "event_count",
            Self::NoteRange { .. } => "note_range",
        }
    }
}

/// Wire form of a rule; converting it into [`InvariantRule`] rejects unknown
/// kinds and misplaced parameters with rule context.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawInvariantRule {
    id: String,
    kind: String,
    #[serde(default = "enabled_default")]
    enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    steps: Option<Vec<u16>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    low: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    high: Option<u8>,
}

fn enabled_default() -> bool {
    true
}

impl TryFrom<RawInvariantRule> for InvariantRule {
    type Error = String;

    fn try_from(raw: RawInvariantRule) -> Result<Self, Self::Error> {
        let id = raw.id;
        let no_steps = |kind: &str| -> Result<(), String> {
            if raw.steps.is_some() {
                return Err(format!(
                    "invariant rule '{id}': kind {kind} does not take 'steps'"
                ));
            }
            Ok(())
        };
        let no_range = |kind: &str| -> Result<(), String> {
            if raw.low.is_some() || raw.high.is_some() {
                return Err(format!(
                    "invariant rule '{id}': kind {kind} does not take 'low'/'high'"
                ));
            }
            Ok(())
        };
        let kind = match raw.kind.as_str() {
            "anchor_steps" => {
                no_range("anchor_steps")?;
                let steps = raw.steps.clone().ok_or_else(|| {
                    format!("invariant rule '{id}': anchor_steps requires 'steps'")
                })?;
                InvariantKind::AnchorSteps { steps }
            }
            "activity_mask" | "pitch_contour" | "event_count" => {
                no_steps(&raw.kind)?;
                no_range(&raw.kind)?;
                match raw.kind.as_str() {
                    "activity_mask" => InvariantKind::ActivityMask,
                    "pitch_contour" => InvariantKind::PitchContour,
                    _ => InvariantKind::EventCount,
                }
            }
            "note_range" => {
                no_steps("note_range")?;
                let (Some(low), Some(high)) = (raw.low, raw.high) else {
                    return Err(format!(
                        "invariant rule '{id}': note_range requires 'low' and 'high'"
                    ));
                };
                InvariantKind::NoteRange { low, high }
            }
            other => {
                return Err(format!(
                    "invariant rule '{id}': unknown invariant kind '{other}'"
                ))
            }
        };
        Ok(Self {
            id,
            enabled: raw.enabled,
            kind,
        })
    }
}

impl From<InvariantRule> for RawInvariantRule {
    fn from(rule: InvariantRule) -> Self {
        let mut raw = RawInvariantRule {
            id: rule.id,
            kind: rule.kind.name().to_string(),
            enabled: rule.enabled,
            steps: None,
            low: None,
            high: None,
        };
        match rule.kind {
            InvariantKind::AnchorSteps { steps } => raw.steps = Some(steps),
            InvariantKind::NoteRange { low, high } => {
                raw.low = Some(low);
                raw.high = Some(high);
            }
            _ => {}
        }
        raw
    }
}

impl InvariantRule {
    pub fn new(id: impl Into<String>, kind: InvariantKind) -> Self {
        Self {
            id: id.into(),
            enabled: true,
            kind,
        }
    }
}

impl InvariantProfile {
    pub fn new(rules: Vec<InvariantRule>) -> Self {
        Self {
            version: INVARIANT_SCHEMA_VERSION,
            rules,
        }
    }

    /// Validate the profile on its own for a pattern of `pattern_len` steps:
    /// schema version, rule IDs (non-empty, unique), one-based anchor steps,
    /// note ranges, and contradictions detectable without pattern contents
    /// (enabled note ranges that do not overlap).
    pub fn validate(&self, pattern_len: usize) -> Result<(), String> {
        if self.version != INVARIANT_SCHEMA_VERSION {
            return Err(format!(
                "unsupported invariant schema version {}; expected {INVARIANT_SCHEMA_VERSION}",
                self.version
            ));
        }
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.id.trim().is_empty() {
                return Err(format!("invariant rule #{} has an empty id", index + 1));
            }
            if rule.id.len() > MAX_RULE_ID_LEN {
                return Err(format!(
                    "invariant rule '{}' id exceeds {MAX_RULE_ID_LEN} bytes",
                    rule.id
                ));
            }
            if self.rules[..index].iter().any(|other| other.id == rule.id) {
                return Err(format!("duplicate invariant rule id '{}'", rule.id));
            }
            match &rule.kind {
                InvariantKind::AnchorSteps { steps } => {
                    if steps.is_empty() {
                        return Err(format!(
                            "invariant rule '{}': anchor_steps needs at least one step",
                            rule.id
                        ));
                    }
                    for (position, &step) in steps.iter().enumerate() {
                        if step == 0 || usize::from(step) > pattern_len {
                            return Err(format!(
                                "invariant rule '{}': anchor step {step} is outside one-based range 1..={pattern_len}",
                                rule.id
                            ));
                        }
                        if steps[..position].contains(&step) {
                            return Err(format!(
                                "invariant rule '{}': anchor step {step} is listed twice",
                                rule.id
                            ));
                        }
                    }
                }
                InvariantKind::NoteRange { low, high } => {
                    if *high > 127 || low > high {
                        return Err(format!(
                            "invariant rule '{}': note range {low}..={high} must satisfy low <= high <= 127",
                            rule.id
                        ));
                    }
                }
                InvariantKind::ActivityMask
                | InvariantKind::PitchContour
                | InvariantKind::EventCount => {}
            }
        }
        // Enabled note ranges must overlap.
        let ranges: Vec<(&str, u8, u8)> = self
            .sorted_enabled_rules()
            .into_iter()
            .filter_map(|rule| match rule.kind {
                InvariantKind::NoteRange { low, high } => Some((rule.id.as_str(), low, high)),
                _ => None,
            })
            .collect();
        for (index, &(id, low, high)) in ranges.iter().enumerate() {
            for &(other_id, other_low, other_high) in &ranges[..index] {
                if low.max(other_low) > high.min(other_high) {
                    return Err(format!(
                        "contradictory invariants '{other_id}' and '{id}': note ranges {other_low}..={other_high} and {low}..={high} do not overlap"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Enabled rules in sorted rule-ID order (the evaluation order).
    pub fn sorted_enabled_rules(&self) -> Vec<&InvariantRule> {
        let mut rules: Vec<&InvariantRule> = self.rules.iter().filter(|r| r.enabled).collect();
        rules.sort_by(|a, b| a.id.cmp(&b.id));
        rules
    }

    pub fn rule(&self, id: &str) -> Option<&InvariantRule> {
        self.rules.iter().find(|rule| rule.id == id)
    }

    fn has_enabled(&self, predicate: impl Fn(&InvariantKind) -> bool) -> bool {
        self.rules
            .iter()
            .any(|rule| rule.enabled && predicate(&rule.kind))
    }

    /// Detect contradictions between enabled rules given the source pattern.
    /// On failure returns the conflicting rule IDs (sorted) and a message.
    pub fn check_contradictions(&self, source: &Pattern) -> Result<(), (Vec<String>, String)> {
        let rules = self.sorted_enabled_rules();
        let ranges = rules.iter().filter_map(|rule| match rule.kind {
            InvariantKind::NoteRange { low, high } => Some((rule.id.as_str(), low, high)),
            _ => None,
        });
        for (range_id, low, high) in ranges {
            for rule in &rules {
                match &rule.kind {
                    InvariantKind::AnchorSteps { steps } => {
                        for &step in steps {
                            let Some(Some(anchored)) =
                                source.steps.get(usize::from(step).wrapping_sub(1))
                            else {
                                continue;
                            };
                            if !(low..=high).contains(&anchored.note) {
                                let mut ids = vec![rule.id.clone(), range_id.to_string()];
                                ids.sort();
                                return Err((
                                    ids,
                                    format!(
                                        "contradictory invariants '{}' and '{range_id}': anchored step {step} note {} is outside note range {low}..={high}",
                                        rule.id, anchored.note
                                    ),
                                ));
                            }
                        }
                    }
                    InvariantKind::PitchContour => {
                        let notes: Vec<u8> =
                            source.steps.iter().flatten().map(|s| s.note).collect();
                        if let (Some(min), Some(max)) = (notes.iter().min(), notes.iter().max()) {
                            if max - min > high - low {
                                let mut ids = vec![rule.id.clone(), range_id.to_string()];
                                ids.sort();
                                return Err((
                                    ids,
                                    format!(
                                        "contradictory invariants '{}' and '{range_id}': contour spans {} semitones but note range {low}..={high} spans {}",
                                        rule.id,
                                        max - min,
                                        high - low
                                    ),
                                ));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Invariant evaluation
// ---------------------------------------------------------------------------

/// Result of evaluating one enabled invariant rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvariantResult {
    pub id: String,
    pub passed: bool,
    pub detail: String,
}

/// Evaluate every enabled rule of `profile` for `candidate` against `source`,
/// in sorted rule-ID order. Pure; never mutates either pattern.
pub fn evaluate_invariants(
    profile: &InvariantProfile,
    source: &Pattern,
    candidate: &Pattern,
) -> Vec<InvariantResult> {
    profile
        .sorted_enabled_rules()
        .into_iter()
        .map(|rule| {
            let outcome = evaluate_rule(&rule.kind, source, candidate);
            let (passed, detail) = match outcome {
                Ok(detail) => (true, detail),
                Err(detail) => (false, detail),
            };
            InvariantResult {
                id: rule.id.clone(),
                passed,
                detail,
            }
        })
        .collect()
}

fn evaluate_rule(
    kind: &InvariantKind,
    source: &Pattern,
    candidate: &Pattern,
) -> Result<String, String> {
    let same_len = || {
        if source.steps.len() == candidate.steps.len() {
            Ok(())
        } else {
            Err(format!(
                "pattern length changed from {} to {}",
                source.steps.len(),
                candidate.steps.len()
            ))
        }
    };
    match kind {
        InvariantKind::AnchorSteps { steps } => {
            for &step in steps {
                let index = usize::from(step).wrapping_sub(1);
                let (Some(before), Some(after)) =
                    (source.steps.get(index), candidate.steps.get(index))
                else {
                    return Err(format!("anchor step {step} is outside the pattern"));
                };
                if before != after {
                    return Err(format!("anchor step {step} changed"));
                }
            }
            Ok(format!("{} anchor step(s) unchanged", steps.len()))
        }
        InvariantKind::ActivityMask => {
            same_len()?;
            match first_activity_mismatch(source, candidate) {
                Some(step) => Err(format!("step {step} active state changed")),
                None => Ok("active/rest positions unchanged".into()),
            }
        }
        InvariantKind::PitchContour => {
            same_len()?;
            if let Some(step) = first_activity_mismatch(source, candidate) {
                return Err(format!("step {step} event/rest position changed"));
            }
            let before = intervals(source);
            let after = intervals(candidate);
            match before.iter().zip(&after).position(|(a, b)| a != b) {
                Some(index) => Err(format!(
                    "interval {} changed from {:+} to {:+} semitones",
                    index + 1,
                    before[index],
                    after[index]
                )),
                None => Ok(format!("{} interval(s) unchanged", before.len())),
            }
        }
        InvariantKind::EventCount => {
            let before = active_count(source);
            let after = active_count(candidate);
            if before == after {
                Ok(format!("{after} active event(s)"))
            } else {
                Err(format!("active events changed from {before} to {after}"))
            }
        }
        InvariantKind::NoteRange { low, high } => {
            for (index, step) in candidate.steps.iter().enumerate() {
                if let Some(step) = step {
                    if !(*low..=*high).contains(&step.note) {
                        return Err(format!(
                            "step {} note {} is outside {low}..={high}",
                            index + 1,
                            step.note
                        ));
                    }
                }
            }
            Ok(format!("all active notes within {low}..={high}"))
        }
    }
}

/// One-based step of the first active/rest mismatch.
fn first_activity_mismatch(source: &Pattern, candidate: &Pattern) -> Option<usize> {
    source
        .steps
        .iter()
        .zip(&candidate.steps)
        .position(|(a, b)| a.is_some() != b.is_some())
        .map(|index| index + 1)
}

fn intervals(pattern: &Pattern) -> Vec<i16> {
    let notes: Vec<i16> = pattern
        .steps
        .iter()
        .flatten()
        .map(|step| i16::from(step.note))
        .collect();
    notes.windows(2).map(|pair| pair[1] - pair[0]).collect()
}

fn active_count(pattern: &Pattern) -> usize {
    pattern.steps.iter().filter(|step| step.is_some()).count()
}

// ---------------------------------------------------------------------------
// PRNG and hashing
// ---------------------------------------------------------------------------

/// SplitMix64 (Steele, Lea & Flood 2014): state += 0x9E3779B97F4A7C15, then
/// the standard 30/27/31 xor-shift-multiply finaliser. Self-contained so the
/// sequence never depends on an external crate version.
#[derive(Debug, Clone)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        mix64(self.state)
    }

    /// Uniform integer in `0..bound` (bound > 0) via 128-bit multiply-high.
    fn below(&mut self, bound: u64) -> u64 {
        ((u128::from(self.next_u64()) * u128::from(bound)) >> 64) as u64
    }

    /// Uniform integer in `low..=high`.
    fn range_inclusive(&mut self, low: i32, high: i32) -> i32 {
        low + self.below((high - low + 1) as u64) as i32
    }

    fn sign(&mut self) -> i32 {
        if self.next_u64() >> 63 == 0 {
            1
        } else {
            -1
        }
    }
}

fn mix64(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Derive a proposal seed when the user omits `--seed`: a SplitMix64 mix of
/// the project seed, editor revision and a monotonic proposal counter.
pub fn derive_seed(project_seed: u64, revision: u64, counter: u64) -> u64 {
    let mut value = mix64(project_seed.wrapping_add(0x9E37_79B9_7F4A_7C15));
    value = mix64(value ^ revision.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    mix64(value ^ counter.wrapping_mul(0x94D0_49BB_1331_11EB))
}

const FNV_OFFSET: u64 = 0xCBF2_9CE4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01B3;

struct Fnv1a(u64);

impl Fnv1a {
    fn bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(FNV_PRIME);
        }
    }
    fn u8(&mut self, value: u8) {
        self.bytes(&[value]);
    }
    fn u16(&mut self, value: u16) {
        self.bytes(&value.to_le_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.bytes(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.bytes(&value.to_le_bytes());
    }
    fn f32(&mut self, value: f32) {
        self.u32(value.to_bits());
    }
    fn str(&mut self, value: &str) {
        self.u32(value.len() as u32);
        self.bytes(value.as_bytes());
    }
}

/// Stable FNV-1a 64 hash of a pattern's canonical byte encoding:
///
/// `"shelloop.pattern.v1"`, name (u32 LE length + UTF-8), seed (u64 LE),
/// swing (f32 bits LE), channel (u8), step count (u32 LE), then per step
/// `0u8` for a rest or `1u8` + note u8 + velocity/gate/probability f32 bits +
/// ratchets u8 + microtiming i32 LE; then lock entry count (u32), per entry
/// step u16 + lock count u32 + per lock (target as compact JSON string, value
/// f32 bits); then `0u8` without invariants or `1u8` + version u32 + rule
/// count u32 + per rule id string, enabled u8, kind tag u8 (0 anchor_steps,
/// 1 activity_mask, 2 pitch_contour, 3 event_count, 4 note_range) and kind
/// parameters (anchor count u32 + u16 steps, or low u8 + high u8).
pub fn pattern_hash(pattern: &Pattern) -> u64 {
    let mut hash = Fnv1a(FNV_OFFSET);
    hash.bytes(b"shelloop.pattern.v1");
    hash.str(&pattern.name);
    hash.u64(pattern.seed);
    hash.f32(pattern.swing);
    hash.u8(pattern.channel);
    hash.u32(pattern.steps.len() as u32);
    for step in &pattern.steps {
        match step {
            None => hash.u8(0),
            Some(step) => {
                hash.u8(1);
                hash.u8(step.note);
                hash.f32(step.velocity);
                hash.f32(step.gate);
                hash.f32(step.probability);
                hash.u8(step.ratchets);
                hash.bytes(&step.microtiming_frames.to_le_bytes());
            }
        }
    }
    hash.u32(pattern.locks.len() as u32);
    for entry in &pattern.locks {
        hash.u16(entry.step);
        hash.u32(entry.locks.len() as u32);
        for lock in &entry.locks {
            let target = serde_json::to_string(&lock.target).unwrap_or_default();
            hash.str(&target);
            hash.f32(lock.value);
        }
    }
    match &pattern.invariants {
        None => hash.u8(0),
        Some(profile) => {
            hash.u8(1);
            hash.u32(profile.version);
            hash.u32(profile.rules.len() as u32);
            for rule in &profile.rules {
                hash.str(&rule.id);
                hash.u8(u8::from(rule.enabled));
                match &rule.kind {
                    InvariantKind::AnchorSteps { steps } => {
                        hash.u8(0);
                        hash.u32(steps.len() as u32);
                        for &step in steps {
                            hash.u16(step);
                        }
                    }
                    InvariantKind::ActivityMask => hash.u8(1),
                    InvariantKind::PitchContour => hash.u8(2),
                    InvariantKind::EventCount => hash.u8(3),
                    InvariantKind::NoteRange { low, high } => {
                        hash.u8(4);
                        hash.u8(*low);
                        hash.u8(*high);
                    }
                }
            }
        }
    }
    hash.0
}

// ---------------------------------------------------------------------------
// Generator
// ---------------------------------------------------------------------------

/// Variation request. `amount` must be finite and within `0.0..=1.0`; it is
/// the proportion of eligible units (steps plus pattern-level swing) that
/// receive one mutation attempt per candidate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VariationRequest {
    pub seed: u64,
    pub amount: f32,
}

/// One changed field. `step` is one-based; `0` denotes pattern-level swing.
/// `field` is one of `swing`, `active`, `note`, `velocity`, `gate`,
/// `probability`, `ratchets`, `microtiming_frames`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldChange {
    pub step: u16,
    pub field: &'static str,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VariationProposal {
    pub seed: u64,
    pub algorithm_version: u32,
    /// Editor revision the proposal was generated from; set by
    /// [`VariationSession::preview`], `None` for direct generator calls.
    pub source_revision: Option<u64>,
    pub source_hash: u64,
    pub candidate_hash: u64,
    /// Candidates tried, including the accepted one (0 for amount zero).
    pub attempts: u32,
    pub changes: Vec<FieldChange>,
    pub invariant_results: Vec<InvariantResult>,
    pub candidate: Pattern,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariationFailure {
    pub attempts: u32,
    /// Rejection counts keyed by rule ID (sorted), plus the pseudo-keys
    /// `pattern_validate` and `no_change`.
    pub rejected_by: Vec<(String, u32)>,
    /// Rule IDs found to contradict each other before any attempt.
    pub conflicting_ids: Vec<String>,
    pub message: String,
}

impl fmt::Display for VariationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl VariationFailure {
    fn invalid(message: impl Into<String>) -> Self {
        Self {
            attempts: 0,
            rejected_by: Vec::new(),
            conflicting_ids: Vec::new(),
            message: message.into(),
        }
    }
}

/// What the locks allow the generator to touch.
#[derive(Debug, Clone, Copy)]
struct MutationScope {
    toggles: bool,
    note_moves: bool,
    note_range: Option<(u8, u8)>,
}

impl MutationScope {
    fn from_profile(profile: Option<&InvariantProfile>) -> Self {
        let Some(profile) = profile else {
            return Self {
                toggles: true,
                note_moves: true,
                note_range: None,
            };
        };
        let toggles = !profile.has_enabled(|kind| {
            matches!(
                kind,
                InvariantKind::ActivityMask
                    | InvariantKind::PitchContour
                    | InvariantKind::EventCount
            )
        });
        let note_moves = !profile.has_enabled(|kind| matches!(kind, InvariantKind::PitchContour));
        // Intersection of enabled ranges (validate guarantees overlap).
        let mut note_range: Option<(u8, u8)> = None;
        for rule in profile.sorted_enabled_rules() {
            if let InvariantKind::NoteRange { low, high } = rule.kind {
                note_range = Some(match note_range {
                    None => (low, high),
                    Some((l, h)) => (l.max(low), h.min(high)),
                });
            }
        }
        Self {
            toggles,
            note_moves,
            note_range,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Swing,
    Step(usize),
}

#[derive(Debug, Clone, Copy)]
enum StepOp {
    Note,
    Velocity,
    Gate,
    Probability,
    Ratchets,
    Microtiming,
    ToggleOff,
}

/// Generate one deterministic variation of `source`.
///
/// Algorithm v1:
/// 1. Reject a non-finite/out-of-range amount, an invalid source, and
///    statically contradictory invariants (no attempts are made).
/// 2. `amount == 0` returns a proposal with no changes (`attempts == 0`,
///    candidate equal to source); such a proposal cannot be accepted.
/// 3. Eligible units are pattern swing plus every step not listed by an
///    enabled anchor rule; rests are eligible only when toggles are allowed
///    (no enabled activity_mask, pitch_contour or event_count rule). Note
///    moves are disabled by an enabled pitch_contour rule.
/// 4. A SplitMix64 PRNG is seeded with `request.seed`. Each attempt clones
///    the source, picks `round(amount * units)` (at least one) distinct units
///    by partial Fisher-Yates, applies one bounded operation to each, and
///    computes the field diff.
/// 5. A candidate is rejected if it changes nothing, fails
///    [`Pattern::validate`], or fails an enabled invariant (sorted ID order);
///    the first passing candidate becomes the proposal. After
///    [`MAX_VARIATION_ATTEMPTS`] rejections a [`VariationFailure`] reports
///    which rules rejected attempts.
///
/// Parameter locks and the invariant profile are carried unchanged.
pub fn generate_variation(
    source: &Pattern,
    request: VariationRequest,
) -> Result<VariationProposal, VariationFailure> {
    if !request.amount.is_finite() || !(0.0..=1.0).contains(&request.amount) {
        return Err(VariationFailure::invalid(format!(
            "variation amount must be finite and between 0 and 1, got {}",
            request.amount
        )));
    }
    source
        .validate()
        .map_err(|error| VariationFailure::invalid(format!("invalid source pattern: {error}")))?;
    let profile = source.invariants.as_ref();
    if let Some(profile) = profile {
        if let Err((ids, message)) = profile.check_contradictions(source) {
            return Err(VariationFailure {
                attempts: 0,
                rejected_by: Vec::new(),
                conflicting_ids: ids,
                message: format!("no valid variation possible: {message}"),
            });
        }
    }
    let evaluate = |candidate: &Pattern| -> Vec<InvariantResult> {
        profile.map_or_else(Vec::new, |profile| {
            evaluate_invariants(profile, source, candidate)
        })
    };
    let source_hash = pattern_hash(source);

    if request.amount == 0.0 {
        return Ok(VariationProposal {
            seed: request.seed,
            algorithm_version: VARIATION_ALGORITHM_VERSION,
            source_revision: None,
            source_hash,
            candidate_hash: source_hash,
            attempts: 0,
            changes: Vec::new(),
            invariant_results: evaluate(source),
            candidate: source.clone(),
        });
    }

    let scope = MutationScope::from_profile(profile);
    let anchored: Vec<bool> = {
        let mut anchored = vec![false; source.steps.len()];
        if let Some(profile) = profile {
            for rule in profile.rules.iter().filter(|rule| rule.enabled) {
                if let InvariantKind::AnchorSteps { steps } = &rule.kind {
                    for &step in steps {
                        if let Some(slot) = anchored.get_mut(usize::from(step).wrapping_sub(1)) {
                            *slot = true;
                        }
                    }
                }
            }
        }
        anchored
    };
    let mut units = vec![Unit::Swing];
    for (index, step) in source.steps.iter().enumerate() {
        if anchored[index] || (step.is_none() && !scope.toggles) {
            continue;
        }
        units.push(Unit::Step(index));
    }
    let target =
        ((f64::from(request.amount) * units.len() as f64).round() as usize).clamp(1, units.len());

    let mut rng = SplitMix64::new(request.seed);
    let mut rejected: Vec<(String, u32)> = Vec::new();
    let mut order: Vec<usize> = Vec::with_capacity(units.len());
    for attempt in 1..=MAX_VARIATION_ATTEMPTS {
        order.clear();
        order.extend(0..units.len());
        for i in 0..target {
            let j = i + rng.below((units.len() - i) as u64) as usize;
            order.swap(i, j);
        }
        let chosen = &mut order[..target];
        chosen.sort_unstable();

        let mut candidate = source.clone();
        for &unit_index in chosen.iter() {
            match units[unit_index] {
                Unit::Swing => candidate.swing = vary_swing(&mut rng, source.swing),
                Unit::Step(index) => mutate_step(&mut rng, source, &mut candidate, index, scope),
            }
        }

        let changes = diff_patterns(source, &candidate);
        if changes.is_empty() {
            bump(&mut rejected, "no_change");
            continue;
        }
        if candidate.validate().is_err() {
            bump(&mut rejected, "pattern_validate");
            continue;
        }
        let results = evaluate(&candidate);
        if results.iter().any(|result| !result.passed) {
            for result in results.iter().filter(|result| !result.passed) {
                bump(&mut rejected, &result.id);
            }
            continue;
        }
        return Ok(VariationProposal {
            seed: request.seed,
            algorithm_version: VARIATION_ALGORITHM_VERSION,
            source_revision: None,
            source_hash,
            candidate_hash: pattern_hash(&candidate),
            attempts: attempt,
            changes,
            invariant_results: results,
            candidate,
        });
    }

    rejected.sort_by(|a, b| a.0.cmp(&b.0));
    let summary = rejected
        .iter()
        .map(|(id, count)| format!("{id} x{count}"))
        .collect::<Vec<_>>()
        .join(", ");
    let lock_rejections = rejected
        .iter()
        .any(|(id, _)| id != "no_change" && id != "pattern_validate");
    let advice = if lock_rejections {
        "; locks rejected every candidate: try reducing --amount or relaxing locks"
    } else {
        "; try a different seed or amount"
    };
    Err(VariationFailure {
        attempts: MAX_VARIATION_ATTEMPTS,
        message: format!(
            "no valid variation found after {MAX_VARIATION_ATTEMPTS} attempts (rejected by: {summary}){advice}"
        ),
        rejected_by: rejected,
        conflicting_ids: Vec::new(),
    })
}

fn bump(counts: &mut Vec<(String, u32)>, id: &str) {
    match counts.iter_mut().find(|(key, _)| key == id) {
        Some((_, count)) => *count += 1,
        None => counts.push((id.to_string(), 1)),
    }
}

/// Round to 0.001 so variations stay readable; deterministic IEEE ops only.
fn quantize_milli(value: f32) -> f32 {
    (value * 1000.0).round() / 1000.0
}

/// Move `old` by `±k * unit` (k in 1..=max_k), clamped to `low..=high`; flips
/// direction when clamping would leave the value unchanged.
fn vary_unit(rng: &mut SplitMix64, old: f32, unit: f32, max_k: i32, low: f32, high: f32) -> f32 {
    let delta = rng.range_inclusive(1, max_k) as f32 * unit * rng.sign() as f32;
    let up = quantize_milli((old + delta).clamp(low, high));
    if up != old {
        return up;
    }
    quantize_milli((old - delta).clamp(low, high))
}

fn vary_swing(rng: &mut SplitMix64, old: f32) -> f32 {
    vary_unit(rng, old, SWING_STEP, 5, 0.0, MAX_SWING)
}

fn mutate_step(
    rng: &mut SplitMix64,
    source: &Pattern,
    candidate: &mut Pattern,
    index: usize,
    scope: MutationScope,
) {
    let Some(step) = source.steps[index] else {
        // Rests are only eligible when toggles are allowed.
        candidate.steps[index] = Some(new_step_from_neighbour(source, index, scope.note_range));
        return;
    };
    let mut ops = Vec::with_capacity(7);
    if scope.note_moves {
        ops.push(StepOp::Note);
    }
    ops.extend([
        StepOp::Velocity,
        StepOp::Gate,
        StepOp::Probability,
        StepOp::Ratchets,
        StepOp::Microtiming,
    ]);
    if scope.toggles {
        ops.push(StepOp::ToggleOff);
    }
    let op = ops[rng.below(ops.len() as u64) as usize];
    let mut next = step;
    match op {
        StepOp::Note => next.note = move_note(rng, step.note, scope.note_range),
        StepOp::Velocity => next.velocity = vary_unit(rng, step.velocity, UNIT_STEP, 4, 0.0, 1.0),
        StepOp::Gate => next.gate = vary_unit(rng, step.gate, UNIT_STEP, 4, 0.0, 1.0),
        StepOp::Probability => {
            next.probability = vary_unit(rng, step.probability, UNIT_STEP, 4, 0.0, 1.0)
        }
        StepOp::Ratchets => {
            next.ratchets = match step.ratchets {
                0 | 1 => 2,
                r if r >= MAX_RATCHETS => MAX_RATCHETS - 1,
                r if rng.sign() > 0 => r + 1,
                r => r - 1,
            }
        }
        StepOp::Microtiming => {
            let delta = rng.range_inclusive(1, 10) * MICROTIMING_STEP_FRAMES * rng.sign();
            let moved = step.microtiming_frames.saturating_add(delta);
            next.microtiming_frames = if moved.unsigned_abs() > MAX_MICROTIMING_FRAMES as u32 {
                step.microtiming_frames.saturating_sub(delta)
            } else {
                moved
            };
        }
        StepOp::ToggleOff => {
            candidate.steps[index] = None;
            return;
        }
    }
    candidate.steps[index] = Some(next);
}

/// Move a note by ±1..=12 semitones within 0..=127. With a note-range lock,
/// prefers moves that land inside the range.
fn move_note(rng: &mut SplitMix64, old: u8, range: Option<(u8, u8)>) -> u8 {
    let (low, high) = range.unwrap_or((0, 127));
    let old = i16::from(old);
    let in_range: Vec<i16> = (-MAX_NOTE_MOVE..=MAX_NOTE_MOVE)
        .filter(|&d| d != 0)
        .map(|d| old + d)
        .filter(|&n| (i16::from(low)..=i16::from(high)).contains(&n))
        .collect();
    if !in_range.is_empty() {
        return in_range[rng.below(in_range.len() as u64) as usize] as u8;
    }
    let delta = rng.range_inclusive(1, i32::from(MAX_NOTE_MOVE)) as i16 * rng.sign() as i16;
    let moved = old + delta;
    let moved = if (0..=127).contains(&moved) {
        moved
    } else {
        old - delta
    };
    moved.clamp(0, 127) as u8
}

/// Value for a newly activated step: note, velocity and gate of the nearest
/// active source neighbour (earlier step wins ties, wrapping around), else
/// note 60 / velocity 0.8 / gate 0.5; probability 1, one ratchet, no
/// microtiming. The note is clamped into an enabled note range.
fn new_step_from_neighbour(source: &Pattern, index: usize, range: Option<(u8, u8)>) -> PatternStep {
    let len = source.steps.len();
    let neighbour = (1..len).find_map(|distance| {
        let before = source.steps[(index + len - distance) % len];
        let after = source.steps[(index + distance) % len];
        before.or(after)
    });
    let (note, velocity, gate) = neighbour.map_or(
        (DEFAULT_NEW_NOTE, DEFAULT_NEW_VELOCITY, DEFAULT_NEW_GATE),
        |step| (step.note, step.velocity, step.gate),
    );
    let note = match range {
        Some((low, high)) => note.clamp(low, high),
        None => note,
    };
    PatternStep {
        note,
        velocity,
        gate,
        probability: 1.0,
        ratchets: 1,
        microtiming_frames: 0,
    }
}

fn format_step(step: &Option<PatternStep>) -> String {
    match step {
        None => "rest".into(),
        Some(step) => format!(
            "note {} vel {:.3} gate {:.3} prob {:.3} ratchets {} micro {}",
            step.note,
            step.velocity,
            step.gate,
            step.probability,
            step.ratchets,
            step.microtiming_frames
        ),
    }
}

/// Field-level diff: swing first (step 0), then steps ascending with fields
/// in a fixed order.
pub fn diff_patterns(source: &Pattern, candidate: &Pattern) -> Vec<FieldChange> {
    let mut changes = Vec::new();
    if source.swing != candidate.swing {
        changes.push(FieldChange {
            step: 0,
            field: "swing",
            before: format!("{:.3}", source.swing),
            after: format!("{:.3}", candidate.swing),
        });
    }
    let len = source.steps.len().max(candidate.steps.len());
    for index in 0..len.min(MAX_PATTERN_STEPS) {
        let before = source.steps.get(index).copied().flatten();
        let after = candidate.steps.get(index).copied().flatten();
        let step = (index + 1) as u16;
        match (before, after) {
            (Some(a), Some(b)) => {
                let mut push = |field: &'static str, before: String, after: String| {
                    if before != after {
                        changes.push(FieldChange {
                            step,
                            field,
                            before,
                            after,
                        });
                    }
                };
                push("note", a.note.to_string(), b.note.to_string());
                push(
                    "velocity",
                    format!("{:.3}", a.velocity),
                    format!("{:.3}", b.velocity),
                );
                push("gate", format!("{:.3}", a.gate), format!("{:.3}", b.gate));
                push(
                    "probability",
                    format!("{:.3}", a.probability),
                    format!("{:.3}", b.probability),
                );
                push("ratchets", a.ratchets.to_string(), b.ratchets.to_string());
                push(
                    "microtiming_frames",
                    a.microtiming_frames.to_string(),
                    b.microtiming_frames.to_string(),
                );
                // Sub-display float differences still count as a change.
                if a != b && changes.last().is_none_or(|c| c.step != step) {
                    changes.push(FieldChange {
                        step,
                        field: "active",
                        before: format_step(&before),
                        after: format_step(&after),
                    });
                }
            }
            (None, None) => {}
            _ => changes.push(FieldChange {
                step,
                field: "active",
                before: format_step(&before),
                after: format_step(&after),
            }),
        }
    }
    changes
}

// ---------------------------------------------------------------------------
// Proposal lifecycle
// ---------------------------------------------------------------------------

/// Per-track proposal holder (control thread). Holds at most one proposal
/// together with the editor revision it was generated from.
#[derive(Debug, Clone, Default)]
pub struct VariationSession {
    proposal: Option<VariationProposal>,
    counter: u64,
}

impl VariationSession {
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed for a preview without `--seed`: advances the monotonic proposal
    /// counter and mixes it with the project seed and editor revision.
    pub fn next_derived_seed(&mut self, project_seed: u64, editor_revision: u64) -> u64 {
        self.counter = self.counter.wrapping_add(1);
        derive_seed(project_seed, editor_revision, self.counter)
    }

    /// Generate a proposal from the editor's current pattern without touching
    /// the editor. A successful preview replaces any prior proposal; a
    /// failed preview leaves the prior proposal in place.
    pub fn preview(
        &mut self,
        editor: &PatternEditor,
        request: VariationRequest,
    ) -> Result<&VariationProposal, VariationFailure> {
        let mut proposal = generate_variation(editor.pattern(), request)?;
        proposal.source_revision = Some(editor.revision());
        Ok(self.proposal.insert(proposal))
    }

    pub fn proposal(&self) -> Option<&VariationProposal> {
        self.proposal.as_ref()
    }

    /// Drop the proposal; returns whether one existed.
    pub fn reject(&mut self) -> bool {
        self.proposal.take().is_some()
    }

    /// Remove the proposal for acceptance. Fails (and discards the
    /// proposal) when the editor revision moved since preview, or when the
    /// proposal contains no changes.
    pub fn take_for_accept(&mut self, editor_revision: u64) -> Result<VariationProposal, String> {
        let proposal = self
            .proposal
            .take()
            .ok_or_else(|| "no variation proposal; run 'variation preview' first".to_string())?;
        if proposal.source_revision != Some(editor_revision) {
            return Err(format!(
                "variation proposal is stale (previewed at revision {}, editor is at {editor_revision}); run 'variation preview' again",
                proposal
                    .source_revision
                    .map_or_else(|| "?".to_string(), |r| r.to_string())
            ));
        }
        if proposal.changes.is_empty() {
            return Err("variation proposal has no changes; nothing to accept".into());
        }
        Ok(proposal)
    }

    /// Put a taken proposal back (e.g. when queueing the accepted revision
    /// failed and the user may retry).
    pub fn restore(&mut self, proposal: VariationProposal) {
        self.proposal = Some(proposal);
    }

    /// Accept into `editor` as one undoable transaction. On an editor error
    /// the proposal is restored and the editor is unchanged.
    pub fn accept(&mut self, editor: &mut PatternEditor) -> Result<u64, String> {
        let proposal = self.take_for_accept(editor.revision())?;
        match editor.replace_pattern(proposal.candidate.clone()) {
            Ok(revision) => Ok(revision),
            Err(error) => {
                self.restore(proposal);
                Err(error)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum VariationCommand {
    /// One-based anchor steps; empty clears the anchor lock.
    LockAnchors(Vec<u16>),
    LockRhythm(bool),
    LockContour(bool),
    LockEventCount(bool),
    LockNoteRange(Option<(u8, u8)>),
    ShowLocks,
    Preview {
        seed: Option<u64>,
        amount: f32,
    },
    Accept,
    Reject,
}

fn parse_on_off(value: Option<&str>, what: &str) -> Result<bool, String> {
    match value {
        Some("on") => Ok(true),
        Some("off") => Ok(false),
        _ => Err(format!("usage: variation lock {what} on|off")),
    }
}

/// Parse a `variation ...` command line.
pub fn parse_variation_command(line: &str) -> Result<VariationCommand, String> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    if tokens.first() != Some(&"variation") {
        return Err("not a variation command".into());
    }
    let only = |count: usize, usage: &str| -> Result<(), String> {
        if tokens.len() == count {
            Ok(())
        } else {
            Err(format!("usage: {usage}"))
        }
    };
    match tokens.get(1).copied() {
        Some("locks") => {
            only(2, "variation locks")?;
            Ok(VariationCommand::ShowLocks)
        }
        Some("accept") => {
            only(2, "variation accept")?;
            Ok(VariationCommand::Accept)
        }
        Some("reject") => {
            only(2, "variation reject")?;
            Ok(VariationCommand::Reject)
        }
        Some("preview") => parse_preview(&tokens[2..]),
        Some("lock") => match tokens.get(2).copied() {
            Some("anchors") => {
                let rest = tokens[3..].join("");
                if rest.is_empty() {
                    return Err("usage: variation lock anchors 1,5,9,13 | off".into());
                }
                if rest == "off" {
                    return Ok(VariationCommand::LockAnchors(Vec::new()));
                }
                let mut steps = Vec::new();
                for part in rest.split(',') {
                    let step: u16 = part
                        .parse()
                        .map_err(|_| format!("invalid anchor step '{part}'"))?;
                    if step == 0 || usize::from(step) > MAX_PATTERN_STEPS {
                        return Err(format!(
                            "anchor step {step} must be one-based within 1..={MAX_PATTERN_STEPS}"
                        ));
                    }
                    if !steps.contains(&step) {
                        steps.push(step);
                    }
                }
                steps.sort_unstable();
                Ok(VariationCommand::LockAnchors(steps))
            }
            Some(kind @ ("rhythm" | "contour" | "count")) => {
                only(4, &format!("variation lock {kind} on|off"))?;
                let on = parse_on_off(tokens.get(3).copied(), kind)?;
                Ok(match kind {
                    "rhythm" => VariationCommand::LockRhythm(on),
                    "contour" => VariationCommand::LockContour(on),
                    _ => VariationCommand::LockEventCount(on),
                })
            }
            Some("note-range") => {
                if tokens.len() == 4 && tokens[3] == "off" {
                    return Ok(VariationCommand::LockNoteRange(None));
                }
                only(5, "variation lock note-range LOW HIGH | off")?;
                let low: u8 = tokens[3]
                    .parse()
                    .map_err(|_| format!("invalid low note '{}'", tokens[3]))?;
                let high: u8 = tokens[4]
                    .parse()
                    .map_err(|_| format!("invalid high note '{}'", tokens[4]))?;
                if high > 127 || low > high {
                    return Err(format!(
                        "note range {low}..={high} must satisfy low <= high <= 127"
                    ));
                }
                Ok(VariationCommand::LockNoteRange(Some((low, high))))
            }
            _ => Err("usage: variation lock anchors|rhythm|contour|count|note-range ...".into()),
        },
        _ => Err(
            "usage: variation lock ... | locks | preview [--seed N] [--amount X] | accept | reject"
                .into(),
        ),
    }
}

fn parse_preview(args: &[&str]) -> Result<VariationCommand, String> {
    let mut seed = None;
    let mut amount = None;
    let mut index = 0;
    while index < args.len() {
        let value = args.get(index + 1).copied();
        match args[index] {
            "--seed" if seed.is_none() => {
                let value = value.ok_or("--seed needs a value")?;
                seed = Some(
                    value
                        .parse::<u64>()
                        .map_err(|_| format!("invalid seed '{value}'"))?,
                );
            }
            "--amount" if amount.is_none() => {
                let value = value.ok_or("--amount needs a value")?;
                let parsed = value
                    .parse::<f32>()
                    .map_err(|_| format!("invalid amount '{value}'"))?;
                if !parsed.is_finite() || !(0.0..=1.0).contains(&parsed) {
                    return Err(format!(
                        "amount must be finite and between 0 and 1, got {value}"
                    ));
                }
                amount = Some(parsed);
            }
            other => {
                return Err(format!(
                    "unexpected '{other}'; usage: variation preview [--seed N] [--amount X]"
                ))
            }
        }
        index += 2;
    }
    Ok(VariationCommand::Preview {
        seed,
        amount: amount.unwrap_or(DEFAULT_VARIATION_AMOUNT),
    })
}

fn upsert_rule(rules: &mut Vec<InvariantRule>, id: &str, kind: InvariantKind) {
    match rules.iter_mut().find(|rule| rule.id == id) {
        Some(rule) => {
            rule.kind = kind;
            rule.enabled = true;
        }
        None => rules.push(InvariantRule::new(id, kind)),
    }
}

/// Apply a `variation lock ...` command to a profile, creating one when
/// needed. Rules use the stable IDs [`ANCHORS_RULE_ID`], [`RHYTHM_RULE_ID`],
/// [`CONTOUR_RULE_ID`], [`COUNT_RULE_ID`] and [`NOTE_RANGE_RULE_ID`];
/// turning a lock off removes its rule. Returns `None` when no rules remain.
pub fn apply_lock_command(
    profile: Option<InvariantProfile>,
    command: &VariationCommand,
) -> Result<Option<InvariantProfile>, String> {
    let (id, kind) = match command {
        VariationCommand::LockAnchors(steps) => (
            ANCHORS_RULE_ID,
            (!steps.is_empty()).then(|| InvariantKind::AnchorSteps {
                steps: steps.clone(),
            }),
        ),
        VariationCommand::LockRhythm(on) => {
            (RHYTHM_RULE_ID, on.then_some(InvariantKind::ActivityMask))
        }
        VariationCommand::LockContour(on) => {
            (CONTOUR_RULE_ID, on.then_some(InvariantKind::PitchContour))
        }
        VariationCommand::LockEventCount(on) => {
            (COUNT_RULE_ID, on.then_some(InvariantKind::EventCount))
        }
        VariationCommand::LockNoteRange(range) => {
            if let Some((low, high)) = range {
                if *high > 127 || low > high {
                    return Err(format!(
                        "note range {low}..={high} must satisfy low <= high <= 127"
                    ));
                }
            }
            (
                NOTE_RANGE_RULE_ID,
                range.map(|(low, high)| InvariantKind::NoteRange { low, high }),
            )
        }
        _ => return Err("not a variation lock command".into()),
    };
    let mut profile = profile.unwrap_or_else(|| InvariantProfile::new(Vec::new()));
    match kind {
        Some(kind) => upsert_rule(&mut profile.rules, id, kind),
        None => profile.rules.retain(|rule| rule.id != id),
    }
    Ok((!profile.rules.is_empty()).then_some(profile))
}

/// Lines for `variation locks`.
pub fn format_locks(profile: Option<&InvariantProfile>) -> Vec<String> {
    let Some(profile) = profile.filter(|profile| !profile.rules.is_empty()) else {
        return vec!["variation locks: none".into()];
    };
    let mut rules: Vec<&InvariantRule> = profile.rules.iter().collect();
    rules.sort_by(|a, b| a.id.cmp(&b.id));
    let mut lines = vec![format!("variation locks (schema v{}):", profile.version)];
    for rule in rules {
        let params = match &rule.kind {
            InvariantKind::AnchorSteps { steps } => format!(
                " steps {}",
                steps
                    .iter()
                    .map(u16::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            InvariantKind::NoteRange { low, high } => format!(" {low}..={high}"),
            _ => String::new(),
        };
        lines.push(format!(
            "  {} [{}] {}{params}",
            rule.id,
            if rule.enabled { "on" } else { "off" },
            rule.kind.name()
        ));
    }
    lines
}

/// Textual diff of a proposal. This is a text preview only; it is not an
/// audible preview.
pub fn format_proposal(proposal: &VariationProposal) -> Vec<String> {
    let mut lines = vec![
        "variation proposal (text preview, not audible)".to_string(),
        format!(
            "seed {}  algorithm v{}  attempts {}",
            proposal.seed, proposal.algorithm_version, proposal.attempts
        ),
    ];
    if let Some(revision) = proposal.source_revision {
        lines.push(format!("source revision {revision}"));
    }
    lines.push(format!(
        "source hash {:016x} -> candidate hash {:016x}",
        proposal.source_hash, proposal.candidate_hash
    ));
    if proposal.changes.is_empty() {
        lines.push("changes: none".into());
    } else {
        lines.push(format!("changes ({}):", proposal.changes.len()));
        for change in &proposal.changes {
            let place = if change.step == 0 {
                "pattern".to_string()
            } else {
                format!("step {}", change.step)
            };
            lines.push(format!(
                "  {place} {}: {} -> {}",
                change.field, change.before, change.after
            ));
        }
    }
    if proposal.invariant_results.is_empty() {
        lines.push("invariants: none".into());
    } else {
        lines.push("invariants:".into());
        for result in &proposal.invariant_results {
            lines.push(format!(
                "  [{}] {}: {}",
                if result.passed { "pass" } else { "FAIL" },
                result.id,
                result.detail
            ));
        }
    }
    lines
}
