use shelloop::params::{
    sample_param_descriptor, synth_param_descriptor, track_param_descriptor, MASTER_GAIN,
};
use shelloop::{
    format_target, parse_learn_command, parse_target, ActionId, ButtonMode, ConflictPolicy,
    EffectLocation, EffectParamId, EffectSlotId, GlobalParamId, LearnCommand, LearnProgress,
    LearnState, MappedOutput, MappingCurve, MappingId, MidiControlMessage, MidiEvent, MidiLearn,
    MidiMapping, MidiPortMatch, MidiSource, ParamCurve, ParamDescriptor, ParameterTarget,
    PickupMode, SampleParamId, SynthParamId, TargetResolver, TrackId, TrackParamId,
    DEFAULT_LEARN_TIMEOUT_MS, MAX_MAPPINGS,
};
use std::cell::RefCell;
use std::collections::HashMap;

const PORT: &str = "nanoKONTROL2 MIDI 1";

/// Fake session controller: descriptors from the shared tables, values
/// stored in a map that tests update by applying outputs.
#[derive(Default)]
struct FakeResolver {
    values: RefCell<HashMap<ParameterTarget, f32>>,
    missing_tracks: Vec<TrackId>,
}

impl FakeResolver {
    fn with(target: ParameterTarget, value: f32) -> Self {
        let resolver = Self::default();
        resolver.set(target, value);
        resolver
    }

    fn set(&self, target: ParameterTarget, value: f32) {
        self.values.borrow_mut().insert(target, value);
    }

    fn get(&self, target: ParameterTarget) -> Option<f32> {
        self.values.borrow().get(&target).copied()
    }

    fn apply(&self, outputs: &[MappedOutput]) {
        for output in outputs {
            if let MappedOutput::SetParameter { target, value } = output {
                self.set(*target, *value);
            }
        }
    }

    fn exists(&self, target: ParameterTarget) -> bool {
        let track = match target {
            ParameterTarget::Track { track, .. }
            | ParameterTarget::Synth { track, .. }
            | ParameterTarget::Sample { track, .. } => Some(track),
            ParameterTarget::Effect {
                location: EffectLocation::Track(track),
                ..
            } => Some(track),
            _ => None,
        };
        track.is_none_or(|track| !self.missing_tracks.contains(&track))
    }
}

impl TargetResolver for FakeResolver {
    fn descriptor(&self, target: ParameterTarget) -> Option<ParamDescriptor> {
        if !self.exists(target) {
            return None;
        }
        match target {
            ParameterTarget::Global(GlobalParamId::MasterGain) => Some(MASTER_GAIN),
            ParameterTarget::Track { param, .. } => Some(track_param_descriptor(param)),
            ParameterTarget::Synth { param, .. } => Some(synth_param_descriptor(param)),
            ParameterTarget::Sample { param, .. } => Some(sample_param_descriptor(param)),
            ParameterTarget::Effect { .. } => Some(ParamDescriptor {
                name: "fx",
                min: 0.0,
                max: 1.0,
                default: 0.0,
                curve: ParamCurve::Linear,
                lockable: true,
            }),
            ParameterTarget::Action(_) => None,
        }
    }

    fn current_value(&self, target: ParameterTarget) -> Option<f32> {
        self.get(target)
    }
}

fn cutoff(track: u16) -> ParameterTarget {
    ParameterTarget::Synth {
        track: TrackId(track),
        param: SynthParamId::FilterCutoff,
    }
}

fn gain(track: u16) -> ParameterTarget {
    ParameterTarget::Track {
        track: TrackId(track),
        param: TrackParamId::Gain,
    }
}

fn mute(track: u16) -> ParameterTarget {
    ParameterTarget::Track {
        track: TrackId(track),
        param: TrackParamId::Mute,
    }
}

fn cc_source(controller: u8) -> MidiSource {
    MidiSource {
        port: MidiPortMatch::Exact { name: PORT.into() },
        channel: Some(0),
        message: MidiControlMessage::ControlChange { controller },
    }
}

fn note_source(note: u8) -> MidiSource {
    MidiSource {
        port: MidiPortMatch::Exact { name: PORT.into() },
        channel: Some(9),
        message: MidiControlMessage::Note { note },
    }
}

fn mapping(
    id: u32,
    source: MidiSource,
    target: ParameterTarget,
    min: f32,
    max: f32,
) -> MidiMapping {
    MidiMapping {
        id: MappingId(id),
        source,
        target,
        min,
        max,
        curve: MappingCurve::Linear,
        inverted: false,
        pickup: PickupMode::Jump,
        button: ButtonMode::Momentary,
        enabled: true,
    }
}

fn cc(controller: u8, value: u8) -> MidiEvent {
    MidiEvent::ControlChange {
        channel: 0,
        controller,
        value,
    }
}

fn note_on(note: u8) -> MidiEvent {
    MidiEvent::NoteOn {
        channel: 9,
        note,
        velocity: 100,
    }
}

fn note_off(note: u8) -> MidiEvent {
    MidiEvent::NoteOff {
        channel: 9,
        note,
        velocity: 0,
    }
}

fn send(
    learn: &mut MidiLearn,
    resolver: &FakeResolver,
    event: MidiEvent,
) -> (LearnProgress, Vec<MappedOutput>) {
    let mut out = Vec::new();
    let progress = learn.handle(PORT, event, 0, resolver, &mut out);
    resolver.apply(&out);
    (progress, out)
}

fn single_value(out: &[MappedOutput]) -> f32 {
    assert_eq!(out.len(), 1, "expected exactly one output, got {out:?}");
    match out[0] {
        MappedOutput::SetParameter { value, .. } => value,
        MappedOutput::Action(action) => panic!("unexpected action {action:?}"),
    }
}

fn assert_close(actual: f32, expected: f32) {
    assert!(
        (actual - expected).abs() <= expected.abs().max(1.0) * 1e-5,
        "expected {expected}, got {actual}"
    );
}

// --- Normalization -----------------------------------------------------------

#[test]
fn cc_normalization_endpoints_and_midpoint() {
    let mut learn = MidiLearn::new(vec![mapping(1, cc_source(7), gain(0), 0.0, 1.0)]).unwrap();
    let resolver = FakeResolver::default();

    let (progress, out) = send(&mut learn, &resolver, cc(7, 0));
    assert_eq!(progress, LearnProgress::NotLearning);
    assert_eq!(single_value(&out), 0.0);

    let (_, out) = send(&mut learn, &resolver, cc(7, 127));
    assert_eq!(single_value(&out), 1.0);

    let (_, out) = send(&mut learn, &resolver, cc(7, 64));
    assert_close(single_value(&out), 64.0 / 127.0);
    let (_, out) = send(&mut learn, &resolver, cc(7, 63));
    assert_close(single_value(&out), 63.0 / 127.0);
}

#[test]
fn pitch_bend_normalizes_with_exact_centre() {
    let mut source = cc_source(0);
    source.message = MidiControlMessage::PitchBend;
    let mut learn = MidiLearn::new(vec![mapping(1, source, gain(0), 0.0, 1.0)]).unwrap();
    let resolver = FakeResolver::default();
    for (raw, expected) in [(-8192, 0.0), (0, 0.5), (8191, 1.0)] {
        let (_, out) = send(
            &mut learn,
            &resolver,
            MidiEvent::PitchBend {
                channel: 0,
                value: raw,
            },
        );
        assert_eq!(single_value(&out), expected, "pitch bend {raw}");
    }
}

// --- Range mapping -----------------------------------------------------------

#[test]
fn linear_log_and_inverted_range_mapping() {
    let mut log = mapping(2, cc_source(2), cutoff(0), 200.0, 8000.0);
    log.curve = MappingCurve::Logarithmic;
    let mut inverted = mapping(3, cc_source(3), gain(1), 0.0, 2.0);
    inverted.inverted = true;
    let mut learn = MidiLearn::new(vec![
        mapping(1, cc_source(1), gain(0), 0.5, 1.5),
        log,
        inverted,
    ])
    .unwrap();
    let resolver = FakeResolver::default();

    // Linear: min + (max - min) * v.
    let (_, out) = send(&mut learn, &resolver, cc(1, 0));
    assert_eq!(single_value(&out), 0.5);
    let (_, out) = send(&mut learn, &resolver, cc(1, 127));
    assert_eq!(single_value(&out), 1.5);

    // Logarithmic: min * (max / min)^v.
    let (_, out) = send(&mut learn, &resolver, cc(2, 0));
    assert_eq!(single_value(&out), 200.0);
    let (_, out) = send(&mut learn, &resolver, cc(2, 127));
    assert_eq!(single_value(&out), 8000.0);
    let (_, out) = send(&mut learn, &resolver, cc(2, 64));
    let v = 64.0f32 / 127.0;
    assert_close(single_value(&out), 200.0 * 40.0f32.powf(v));

    // Inverted flips the normalized value before mapping.
    let (_, out) = send(&mut learn, &resolver, cc(3, 0));
    assert_eq!(single_value(&out), 2.0);
    let (_, out) = send(&mut learn, &resolver, cc(3, 127));
    assert_eq!(single_value(&out), 0.0);
}

#[test]
fn logarithmic_with_non_positive_endpoint_falls_back_to_linear() {
    let mut pan = mapping(1, cc_source(1), gain(0), 0.0, 2.0);
    pan.curve = MappingCurve::Logarithmic;
    let mut learn = MidiLearn::new(vec![pan]).unwrap();
    let resolver = FakeResolver::default();
    let (_, out) = send(&mut learn, &resolver, cc(1, 127));
    assert_eq!(single_value(&out), 2.0);
    let v = 32.0f32 / 127.0;
    let (_, out) = send(&mut learn, &resolver, cc(1, 32));
    assert_close(single_value(&out), 2.0 * v);
}

#[test]
fn discrete_targets_are_rounded() {
    let semitone = ParameterTarget::Synth {
        track: TrackId(0),
        param: SynthParamId::Semitone,
    };
    let mut learn = MidiLearn::new(vec![mapping(1, cc_source(1), semitone, -12.0, 12.0)]).unwrap();
    let resolver = FakeResolver::default();
    for value in 0..=127u8 {
        let (_, out) = send(&mut learn, &resolver, cc(1, value));
        let mapped = single_value(&out);
        assert_eq!(mapped, mapped.round());
        assert!((-12.0..=12.0).contains(&mapped));
    }
}

// --- Pickup / Jump / Scale ---------------------------------------------------

#[test]
fn pickup_ignores_values_until_crossing_target() {
    let mut map = mapping(1, cc_source(1), gain(0), 0.0, 2.0);
    map.pickup = PickupMode::Pickup;
    let mut learn = MidiLearn::new(vec![map]).unwrap();
    // Software at 1.0 -> normalized 0.5.
    let resolver = FakeResolver::with(gain(0), 1.0);

    for value in [0, 10, 30, 50] {
        let (_, out) = send(&mut learn, &resolver, cc(1, value));
        assert!(
            out.is_empty(),
            "value {value} must be ignored before pickup"
        );
    }
    assert_eq!(resolver.get(gain(0)), Some(1.0));

    // 70 crosses 63.5 -> picked up and applied.
    let (_, out) = send(&mut learn, &resolver, cc(1, 70));
    assert_close(single_value(&out), 2.0 * 70.0 / 127.0);

    // Now follows in both directions.
    let (_, out) = send(&mut learn, &resolver, cc(1, 10));
    assert_close(single_value(&out), 2.0 * 10.0 / 127.0);
}

#[test]
fn pickup_engages_when_value_equal_within_one_step() {
    let mut map = mapping(1, cc_source(1), gain(0), 0.0, 2.0);
    map.pickup = PickupMode::Pickup;
    let mut learn = MidiLearn::new(vec![map]).unwrap();
    let resolver = FakeResolver::with(gain(0), 1.0);
    // First message ever, 64/127 is within 1/127 of 0.5.
    let (_, out) = send(&mut learn, &resolver, cc(1, 64));
    assert_eq!(out.len(), 1);
}

#[test]
fn pickup_rearms_after_external_change() {
    let mut map = mapping(1, cc_source(1), gain(0), 0.0, 2.0);
    map.pickup = PickupMode::Pickup;
    let mut learn = MidiLearn::new(vec![map]).unwrap();
    let resolver = FakeResolver::with(gain(0), 0.0);
    let (_, out) = send(&mut learn, &resolver, cc(1, 0));
    assert_eq!(out.len(), 1);
    let (_, out) = send(&mut learn, &resolver, cc(1, 20));
    assert_eq!(out.len(), 1);

    // UI moves the value to the top; pickup must re-arm.
    resolver.set(gain(0), 2.0);
    learn.invalidate_pickup(gain(0));
    let (_, out) = send(&mut learn, &resolver, cc(1, 30));
    assert!(out.is_empty());
    let (_, out) = send(&mut learn, &resolver, cc(1, 127));
    assert_eq!(single_value(&out), 2.0);
}

#[test]
fn jump_mode_applies_immediately() {
    let mut learn = MidiLearn::new(vec![mapping(1, cc_source(1), gain(0), 0.0, 2.0)]).unwrap();
    let resolver = FakeResolver::with(gain(0), 1.0);
    let (_, out) = send(&mut learn, &resolver, cc(1, 0));
    assert_eq!(
        out,
        vec![MappedOutput::SetParameter {
            target: gain(0),
            value: 0.0
        }]
    );
}

#[test]
fn scale_mode_converges_without_jumping() {
    let mut map = mapping(1, cc_source(1), gain(0), 0.0, 1.0);
    map.pickup = PickupMode::Scale;
    let mut learn = MidiLearn::new(vec![map]).unwrap();
    let resolver = FakeResolver::with(gain(0), 0.8);

    // First touch establishes the hardware position only.
    let (_, out) = send(&mut learn, &resolver, cc(1, 0));
    assert!(out.is_empty());

    // Moving up maps remaining hardware travel (1 - 0) onto remaining
    // software travel (1 - 0.8): small steps, never a jump down.
    let mut previous = 0.8f32;
    for value in (10..=127).step_by(10).chain([127]) {
        let (_, out) = send(&mut learn, &resolver, cc(1, value));
        let mapped = single_value(&out);
        assert!(mapped >= previous - 1e-6, "{mapped} < {previous}");
        assert!(mapped - previous <= 0.2 + 1e-6);
        previous = mapped;
    }
    assert_eq!(resolver.get(gain(0)), Some(1.0));

    // Once converged it follows the hardware directly.
    let (_, out) = send(&mut learn, &resolver, cc(1, 0));
    assert_eq!(single_value(&out), 0.0);
}

// --- Buttons -----------------------------------------------------------------

#[test]
fn toggle_reacts_once_per_press_and_ignores_releases() {
    let mut map = mapping(1, note_source(36), mute(0), 0.0, 1.0);
    map.button = ButtonMode::Toggle;
    let mut learn = MidiLearn::new(vec![map]).unwrap();
    let resolver = FakeResolver::with(mute(0), 0.0);

    let (_, out) = send(&mut learn, &resolver, note_on(36));
    assert_eq!(single_value(&out), 1.0);
    // Repeated press without release: ignored.
    let (_, out) = send(&mut learn, &resolver, note_on(36));
    assert!(out.is_empty());
    // Releases (including repeated ones) never toggle.
    for _ in 0..3 {
        let (_, out) = send(&mut learn, &resolver, note_off(36));
        assert!(out.is_empty());
    }
    let (_, out) = send(&mut learn, &resolver, note_on(36));
    assert_eq!(single_value(&out), 0.0);
    let (_, out) = send(&mut learn, &resolver, note_off(36));
    assert!(out.is_empty());
    let (_, out) = send(&mut learn, &resolver, note_on(36));
    assert_eq!(single_value(&out), 1.0);
}

#[test]
fn toggle_on_cc_button_uses_threshold() {
    let mut map = mapping(1, cc_source(40), mute(0), 0.0, 1.0);
    map.button = ButtonMode::Toggle;
    let mut learn = MidiLearn::new(vec![map]).unwrap();
    let resolver = FakeResolver::with(mute(0), 1.0);
    let (_, out) = send(&mut learn, &resolver, cc(40, 127));
    assert_eq!(single_value(&out), 0.0);
    let (_, out) = send(&mut learn, &resolver, cc(40, 100));
    assert!(out.is_empty());
    let (_, out) = send(&mut learn, &resolver, cc(40, 0));
    assert!(out.is_empty());
    let (_, out) = send(&mut learn, &resolver, cc(40, 64));
    assert_eq!(single_value(&out), 1.0);
}

#[test]
fn momentary_and_trigger_buttons() {
    let mut momentary = mapping(1, note_source(36), mute(0), 0.0, 1.0);
    momentary.button = ButtonMode::Momentary;
    let mut trigger = mapping(2, note_source(37), gain(0), 0.0, 2.0);
    trigger.button = ButtonMode::Trigger;
    let mut learn = MidiLearn::new(vec![momentary, trigger]).unwrap();
    let resolver = FakeResolver::default();

    let (_, out) = send(&mut learn, &resolver, note_on(36));
    assert_eq!(single_value(&out), 1.0);
    let (_, out) = send(&mut learn, &resolver, note_off(36));
    assert_eq!(single_value(&out), 0.0);

    let (_, out) = send(&mut learn, &resolver, note_on(37));
    assert_eq!(single_value(&out), 2.0);
    let (_, out) = send(&mut learn, &resolver, note_off(37));
    assert!(out.is_empty());
}

#[test]
fn action_targets_emit_actions_on_press_only() {
    let play = ParameterTarget::Action(ActionId::TogglePlay);
    let scene = ParameterTarget::Action(ActionId::SceneLaunch(3));
    let mut learn = MidiLearn::new(vec![
        mapping(1, note_source(40), play, 0.0, 1.0),
        mapping(2, cc_source(41), scene, 0.0, 1.0),
    ])
    .unwrap();
    let resolver = FakeResolver::default();

    let (_, out) = send(&mut learn, &resolver, note_on(40));
    assert_eq!(out, vec![MappedOutput::Action(ActionId::TogglePlay)]);
    let (_, out) = send(&mut learn, &resolver, note_off(40));
    assert!(out.is_empty());

    let (_, out) = send(&mut learn, &resolver, cc(41, 127));
    assert_eq!(out, vec![MappedOutput::Action(ActionId::SceneLaunch(3))]);
    // Holding/sweeping above the threshold does not retrigger.
    let (_, out) = send(&mut learn, &resolver, cc(41, 100));
    assert!(out.is_empty());
    let (_, out) = send(&mut learn, &resolver, cc(41, 0));
    assert!(out.is_empty());
    let (_, out) = send(&mut learn, &resolver, cc(41, 127));
    assert_eq!(out.len(), 1);
}

// --- Learn workflow ----------------------------------------------------------

#[test]
fn learn_captures_source_without_mutating_target() {
    let existing = mapping(1, cc_source(74), gain(3), 0.0, 2.0);
    let mut learn = MidiLearn::new(vec![existing.clone()]).unwrap();
    let resolver = FakeResolver::with(cutoff(2), 440.0);
    resolver.set(gain(3), 1.0);

    learn.begin_learn(cutoff(2), 1_000, false);
    assert_eq!(
        learn.learn_state(),
        &LearnState::Awaiting {
            target: cutoff(2),
            deadline_ms: 1_000 + DEFAULT_LEARN_TIMEOUT_MS,
            allow_notes: false,
        }
    );

    // Notes are excluded by default.
    let mut out = Vec::new();
    let progress = learn.handle(PORT, note_on(36), 1_100, &resolver, &mut out);
    assert_eq!(progress, LearnProgress::Ignored);
    assert!(out.is_empty());

    // A CC already mapped to another target is captured, not applied.
    let event = MidiEvent::ControlChange {
        channel: 4,
        controller: 21,
        value: 99,
    };
    let progress = learn.handle(PORT, event, 1_200, &resolver, &mut out);
    assert_eq!(progress, LearnProgress::Captured { conflicts: vec![] });
    assert!(out.is_empty());
    assert_eq!(
        learn.learn_state(),
        &LearnState::Captured {
            target: cutoff(2),
            source: MidiSource {
                port: MidiPortMatch::Exact { name: PORT.into() },
                channel: Some(4),
                message: MidiControlMessage::ControlChange { controller: 21 },
            },
            conflicts: vec![],
        }
    );
    assert_eq!(resolver.get(cutoff(2)), Some(440.0));
    assert_eq!(learn.mappings(), std::slice::from_ref(&existing));

    // Further motion of the captured control is swallowed until confirmed.
    let progress = learn.handle(PORT, event, 1_300, &resolver, &mut out);
    assert_eq!(progress, LearnProgress::Ignored);
    assert!(out.is_empty());
    assert_eq!(learn.mappings(), &[existing]);

    let id = learn
        .confirm(ConflictPolicy::Replace, &resolver)
        .unwrap()
        .unwrap();
    assert_eq!(learn.learn_state(), &LearnState::Idle);
    let created = learn.mappings().iter().find(|m| m.id == id).unwrap();
    assert_eq!(created.target, cutoff(2));
    assert_eq!(created.min, 20.0);
    assert_eq!(created.max, 20_000.0);
    assert_eq!(created.curve, MappingCurve::Logarithmic);
    assert_eq!(created.pickup, PickupMode::Pickup);
    assert_eq!(created.button, ButtonMode::Momentary);
    assert!(created.enabled);
    assert_eq!(learn.mappings().len(), 2);
}

#[test]
fn learn_notes_when_enabled_defaults_to_toggle_or_trigger() {
    let mut learn = MidiLearn::new(vec![]).unwrap();
    let resolver = FakeResolver::default();

    learn.begin_learn(mute(1), 0, true);
    let (progress, _) = send(&mut learn, &resolver, note_off(36));
    assert_eq!(progress, LearnProgress::Ignored);
    let (progress, _) = send(&mut learn, &resolver, note_on(36));
    assert_eq!(progress, LearnProgress::Captured { conflicts: vec![] });
    let id = learn
        .confirm(ConflictPolicy::Replace, &resolver)
        .unwrap()
        .unwrap();
    let m = learn
        .mappings()
        .iter()
        .find(|m| m.id == id)
        .unwrap()
        .clone();
    assert_eq!(m.pickup, PickupMode::Jump);
    assert_eq!(m.button, ButtonMode::Toggle);
    assert_eq!((m.min, m.max), (0.0, 1.0));

    let scene = ParameterTarget::Action(ActionId::SceneNext);
    learn.begin_learn(scene, 0, true);
    send(&mut learn, &resolver, note_on(37));
    let id = learn
        .confirm(ConflictPolicy::Replace, &resolver)
        .unwrap()
        .unwrap();
    let m = learn.mappings().iter().find(|m| m.id == id).unwrap();
    assert_eq!(m.button, ButtonMode::Trigger);
    assert_eq!(learn.mappings().len(), 2);
}

#[test]
fn learn_timeout_leaves_mappings_unchanged() {
    let existing = vec![mapping(1, cc_source(1), gain(0), 0.0, 2.0)];
    let mut learn = MidiLearn::new(existing.clone()).unwrap();
    let resolver = FakeResolver::default();
    learn.begin_learn(gain(1), 500, false);
    assert!(!learn.tick(500 + DEFAULT_LEARN_TIMEOUT_MS - 1));
    assert!(matches!(learn.learn_state(), LearnState::Awaiting { .. }));
    assert!(learn.tick(500 + DEFAULT_LEARN_TIMEOUT_MS));
    assert_eq!(learn.learn_state(), &LearnState::Idle);
    assert!(!learn.tick(1_000_000));
    assert_eq!(learn.mappings(), existing.as_slice());
    assert!(learn.confirm(ConflictPolicy::Replace, &resolver).is_err());

    // A message after the deadline is applied normally, not captured.
    learn.begin_learn(gain(1), 0, false);
    let mut out = Vec::new();
    let progress = learn.handle(
        PORT,
        cc(1, 127),
        DEFAULT_LEARN_TIMEOUT_MS,
        &resolver,
        &mut out,
    );
    assert_eq!(progress, LearnProgress::NotLearning);
    assert_eq!(out.len(), 1);
    assert_eq!(learn.learn_state(), &LearnState::Idle);
    assert_eq!(learn.mappings(), existing.as_slice());
}

#[test]
fn learn_cancel_leaves_mappings_unchanged() {
    let existing = vec![mapping(1, cc_source(1), gain(0), 0.0, 2.0)];
    let mut learn = MidiLearn::new(existing.clone()).unwrap();
    let resolver = FakeResolver::default();

    learn.begin_learn(gain(1), 0, false);
    learn.cancel_learn();
    assert_eq!(learn.learn_state(), &LearnState::Idle);

    learn.begin_learn(gain(1), 0, false);
    send(&mut learn, &resolver, cc(9, 10));
    assert!(matches!(learn.learn_state(), LearnState::Captured { .. }));
    learn.cancel_learn();
    assert_eq!(learn.mappings(), existing.as_slice());

    learn.begin_learn(gain(1), 0, false);
    send(&mut learn, &resolver, cc(9, 10));
    assert_eq!(learn.confirm(ConflictPolicy::Cancel, &resolver), Ok(None));
    assert_eq!(learn.learn_state(), &LearnState::Idle);
    assert_eq!(learn.mappings(), existing.as_slice());
}

#[test]
fn existing_source_conflict_follows_explicit_policy() {
    let existing = mapping(5, cc_source(74), gain(0), 0.0, 2.0);
    let resolver = FakeResolver::default();

    // Replace removes the old binding.
    let mut learn = MidiLearn::new(vec![existing.clone()]).unwrap();
    learn.begin_learn(cutoff(0), 0, false);
    let (progress, out) = send(&mut learn, &resolver, cc(74, 3));
    assert!(out.is_empty());
    assert_eq!(
        progress,
        LearnProgress::Captured {
            conflicts: vec![MappingId(5)]
        }
    );
    let id = learn
        .confirm(ConflictPolicy::Replace, &resolver)
        .unwrap()
        .unwrap();
    assert_eq!(learn.mappings().len(), 1);
    assert_eq!(learn.mappings()[0].id, id);
    assert_eq!(learn.mappings()[0].target, cutoff(0));
    assert!(id > MappingId(5));

    // AddSecondary keeps both; one source now drives two targets.
    let mut learn = MidiLearn::new(vec![existing.clone()]).unwrap();
    learn.begin_learn(cutoff(0), 0, false);
    send(&mut learn, &resolver, cc(74, 3));
    learn
        .confirm(ConflictPolicy::AddSecondary, &resolver)
        .unwrap()
        .unwrap();
    assert_eq!(learn.mappings().len(), 2);
    learn.set_pickup(MappingId(6), PickupMode::Jump).unwrap();
    let (_, out) = send(&mut learn, &resolver, cc(74, 127));
    assert_eq!(out.len(), 2);

    // Cancel changes nothing.
    let mut learn = MidiLearn::new(vec![existing.clone()]).unwrap();
    learn.begin_learn(cutoff(0), 0, false);
    send(&mut learn, &resolver, cc(74, 3));
    assert_eq!(learn.confirm(ConflictPolicy::Cancel, &resolver), Ok(None));
    assert_eq!(learn.mappings(), &[existing]);
}

#[test]
fn multiple_sources_may_share_a_target() {
    let mut learn = MidiLearn::new(vec![
        mapping(1, cc_source(1), gain(0), 0.0, 2.0),
        mapping(2, cc_source(2), gain(0), 0.0, 2.0),
    ])
    .unwrap();
    let resolver = FakeResolver::default();
    let (_, out) = send(&mut learn, &resolver, cc(1, 127));
    assert_eq!(single_value(&out), 2.0);
    let (_, out) = send(&mut learn, &resolver, cc(2, 0));
    assert_eq!(single_value(&out), 0.0);
}

#[test]
fn shared_target_rearms_pickup_of_other_mapping() {
    let mut a = mapping(1, cc_source(1), gain(0), 0.0, 2.0);
    a.pickup = PickupMode::Pickup;
    let mut b = mapping(2, cc_source(2), gain(0), 0.0, 2.0);
    b.pickup = PickupMode::Pickup;
    let mut learn = MidiLearn::new(vec![a, b]).unwrap();
    let resolver = FakeResolver::with(gain(0), 0.0);
    // Both pick up at zero.
    assert_eq!(send(&mut learn, &resolver, cc(1, 0)).1.len(), 1);
    assert_eq!(send(&mut learn, &resolver, cc(2, 0)).1.len(), 1);
    // A moves the target to the top; B must not jump it back down.
    assert_eq!(send(&mut learn, &resolver, cc(1, 127)).1.len(), 1);
    assert!(send(&mut learn, &resolver, cc(2, 5)).1.is_empty());
}

// --- Orphans, ports, determinism -------------------------------------------

#[test]
fn deleted_track_leaves_mapping_orphaned_never_retargeted() {
    let mappings = vec![
        mapping(1, cc_source(1), gain(2), 0.0, 2.0),
        mapping(2, cc_source(2), cutoff(2), 20.0, 20_000.0),
        mapping(3, cc_source(3), gain(3), 0.0, 2.0),
    ];
    let mut learn = MidiLearn::new(mappings.clone()).unwrap();
    let resolver = FakeResolver {
        missing_tracks: vec![TrackId(2)],
        ..FakeResolver::default()
    };

    let orphans = learn.disable_orphans(|target| resolver.exists(target));
    assert_eq!(orphans, vec![MappingId(1), MappingId(2)]);
    for (before, after) in mappings.iter().zip(learn.mappings()) {
        assert_eq!(before.target, after.target, "never retargeted");
        assert_eq!(before.source, after.source);
    }
    assert!(!learn.mappings()[0].enabled);
    assert!(!learn.mappings()[1].enabled);
    assert!(learn.mappings()[2].enabled);

    let (_, out) = send(&mut learn, &resolver, cc(1, 127));
    assert!(out.is_empty());
    let (_, out) = send(&mut learn, &resolver, cc(3, 127));
    assert_eq!(out.len(), 1);

    // Disabled orphans survive persistence as disabled.
    let json = serde_json::to_string(learn.mappings()).unwrap();
    let restored: Vec<MidiMapping> = serde_json::from_str(&json).unwrap();
    assert!(!restored[0].enabled);
    assert_eq!(restored[0].target, gain(2));
}

#[test]
fn port_disconnect_reconnect_preserves_mapping_definition() {
    let mappings = vec![mapping(1, cc_source(7), gain(0), 0.0, 2.0)];
    let mut learn = MidiLearn::new(mappings.clone()).unwrap();
    let resolver = FakeResolver::default();

    let mut out = Vec::new();
    learn.handle(PORT, cc(7, 127), 0, &resolver, &mut out);
    assert_eq!(out.len(), 1);

    // "Disconnect": a different device never matches an exact mapping.
    out.clear();
    learn.handle("Other Device", cc(7, 0), 10, &resolver, &mut out);
    assert!(out.is_empty());
    assert_eq!(learn.mappings(), mappings.as_slice());

    // Reconnect with the same port name string: the mapping still matches.
    let reconnected_name = String::from("nanoKONTROL2 MIDI 1");
    out.clear();
    learn.handle(&reconnected_name, cc(7, 0), 20, &resolver, &mut out);
    assert_eq!(
        out,
        vec![MappedOutput::SetParameter {
            target: gain(0),
            value: 0.0
        }]
    );
    assert_eq!(learn.mappings(), mappings.as_slice());
}

#[test]
fn any_port_and_any_channel_match_only_when_chosen() {
    let mut any = mapping(1, cc_source(7), gain(0), 0.0, 1.0);
    any.source.port = MidiPortMatch::Any;
    any.source.channel = None;
    let mut learn = MidiLearn::new(vec![any, mapping(2, cc_source(8), gain(1), 0.0, 1.0)]).unwrap();
    let resolver = FakeResolver::default();
    let mut out = Vec::new();
    let event = MidiEvent::ControlChange {
        channel: 11,
        controller: 7,
        value: 127,
    };
    learn.handle("Whatever", event, 0, &resolver, &mut out);
    assert_eq!(out.len(), 1);
    out.clear();
    let event = MidiEvent::ControlChange {
        channel: 11,
        controller: 8,
        value: 127,
    };
    learn.handle(PORT, event, 0, &resolver, &mut out);
    assert!(out.is_empty(), "channel mismatch must not match");
}

#[test]
fn compiled_lookup_is_deterministic_and_bounded() {
    // Insert in shuffled id order; outputs must come out in id order.
    let mut mappings = Vec::new();
    for id in [9u32, 3, 7, 1, 5] {
        mappings.push(mapping(id, cc_source(10), gain(id as u16), 0.0, 1.0));
    }
    let resolver = FakeResolver::default();
    let mut first = None;
    for _ in 0..3 {
        let mut learn = MidiLearn::new(mappings.clone()).unwrap();
        let ids: Vec<_> = learn.mappings().iter().map(|m| m.id.0).collect();
        assert_eq!(ids, vec![1, 3, 5, 7, 9]);
        let (_, out) = send(&mut learn, &resolver, cc(10, 127));
        assert_eq!(out.len(), 5, "one output per matching mapping");
        let targets: Vec<_> = out
            .iter()
            .map(|o| match o {
                MappedOutput::SetParameter { target, .. } => *target,
                MappedOutput::Action(_) => unreachable!(),
            })
            .collect();
        assert_eq!(targets, vec![gain(1), gain(3), gain(5), gain(7), gain(9)]);
        if let Some(previous) = &first {
            assert_eq!(previous, &out);
        }
        first = Some(out);
    }

    // Bounded: at most MAX_MAPPINGS mappings, so at most that many outputs.
    let full: Vec<_> = (1..=MAX_MAPPINGS as u32)
        .map(|id| mapping(id, cc_source(10), gain(0), 0.0, 1.0))
        .collect();
    let mut learn = MidiLearn::new(full.clone()).unwrap();
    let (_, out) = send(&mut learn, &resolver, cc(10, 127));
    assert_eq!(out.len(), MAX_MAPPINGS);
    let mut too_many = full;
    too_many.push(mapping(
        MAX_MAPPINGS as u32 + 1,
        cc_source(1),
        gain(0),
        0.0,
        1.0,
    ));
    assert!(MidiLearn::new(too_many).is_err());

    // Learning beyond capacity is refused without changing mappings.
    learn.begin_learn(gain(1), 0, false);
    send(&mut learn, &resolver, cc(99, 1));
    assert!(learn
        .confirm(ConflictPolicy::AddSecondary, &resolver)
        .is_err());
    assert_eq!(learn.mappings().len(), MAX_MAPPINGS);
}

// --- Validation, editing, persistence ----------------------------------------

#[test]
fn new_rejects_invalid_or_duplicate_mappings() {
    let ok = mapping(1, cc_source(1), gain(0), 0.0, 1.0);
    assert!(MidiLearn::new(vec![ok.clone(), ok.clone()]).is_err());

    let mut bad = ok.clone();
    bad.min = f32::NAN;
    assert!(bad.validate().is_err());
    let mut bad = ok.clone();
    bad.source.channel = Some(16);
    assert!(bad.validate().is_err());
    let mut bad = ok.clone();
    bad.source.message = MidiControlMessage::ControlChange { controller: 128 };
    assert!(bad.validate().is_err());
    let mut bad = ok.clone();
    bad.source.port = MidiPortMatch::Exact {
        name: String::new(),
    };
    assert!(bad.validate().is_err());
    assert!(MidiLearn::new(vec![bad]).is_err());
    assert!(ok.validate().is_ok());
}

#[test]
fn mapping_edits_validate_and_apply() {
    let mut learn =
        MidiLearn::new(vec![mapping(1, cc_source(1), cutoff(0), 20.0, 20_000.0)]).unwrap();
    let resolver = FakeResolver::default();
    learn.set_range(MappingId(1), 200.0, 8000.0).unwrap();
    assert!(learn.set_range(MappingId(1), f32::INFINITY, 1.0).is_err());
    assert!(learn.set_range(MappingId(42), 0.0, 1.0).is_err());
    learn.set_inverted(MappingId(1), true).unwrap();
    learn
        .set_button(MappingId(1), ButtonMode::Momentary)
        .unwrap();
    learn.set_pickup(MappingId(1), PickupMode::Jump).unwrap();
    let (_, out) = send(&mut learn, &resolver, cc(1, 127));
    assert_eq!(single_value(&out), 200.0);

    let summary = learn.describe(MappingId(1)).unwrap();
    assert!(summary.contains("#1"), "{summary}");
    assert!(summary.contains("cc 1"), "{summary}");
    assert!(summary.contains("track 0 filter.cutoff"), "{summary}");
    assert!(summary.contains("inverted"), "{summary}");
    assert!(learn.describe(MappingId(2)).is_none());

    assert!(learn.remove(MappingId(1)));
    assert!(!learn.remove(MappingId(1)));
    assert!(learn.mappings().is_empty());
    let (_, out) = send(&mut learn, &resolver, cc(1, 127));
    assert!(out.is_empty());
}

#[test]
fn mappings_roundtrip_through_json() {
    let mut log = mapping(2, cc_source(74), cutoff(1), 200.0, 8000.0);
    log.curve = MappingCurve::Logarithmic;
    log.inverted = true;
    log.pickup = PickupMode::Scale;
    let mut pad = mapping(
        3,
        note_source(36),
        ParameterTarget::Action(ActionId::SceneLaunch(4)),
        0.0,
        1.0,
    );
    pad.button = ButtonMode::Trigger;
    pad.source.port = MidiPortMatch::Any;
    pad.source.channel = None;
    let mut bend = mapping(
        4,
        cc_source(0),
        ParameterTarget::Effect {
            location: EffectLocation::Master,
            slot: EffectSlotId(0),
            param: EffectParamId(4),
        },
        0.0,
        1.0,
    );
    bend.source.message = MidiControlMessage::PitchBend;
    bend.enabled = false;
    let mappings = vec![mapping(1, cc_source(7), gain(0), 0.0, 2.0), log, pad, bend];

    let json = serde_json::to_string_pretty(&mappings).unwrap();
    assert!(json.contains("\"control_change\""), "{json}");
    assert!(json.contains("\"pitch_bend\""), "{json}");
    assert!(json.contains("\"logarithmic\""), "{json}");
    assert!(json.contains("\"scale\""), "{json}");
    let restored: Vec<MidiMapping> = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, mappings);
    let learn = MidiLearn::new(restored).unwrap();
    assert_eq!(learn.mappings(), mappings.as_slice());
}

#[test]
fn missing_enabled_field_defaults_to_enabled() {
    let json = r#"{"id":1,"source":{"port":"any","channel":null,"message":{"type":"control_change","controller":7}},"target":{"global":"master_gain"},"min":0.0,"max":2.0,"curve":"linear","inverted":false,"pickup":"pickup","button":"momentary"}"#;
    let restored: MidiMapping = serde_json::from_str(json).unwrap();
    assert!(restored.enabled);
    assert_eq!(restored.source.port, MidiPortMatch::Any);
}

// --- Text commands and targets -------------------------------------------------

#[test]
fn parse_targets() {
    let sel = TrackId(1);
    let cases = [
        (
            "master.gain",
            ParameterTarget::Global(GlobalParamId::MasterGain),
        ),
        ("track.gain", gain(1)),
        (
            "track.pan",
            ParameterTarget::Track {
                track: sel,
                param: TrackParamId::Pan,
            },
        ),
        (
            "track.send_b",
            ParameterTarget::Track {
                track: sel,
                param: TrackParamId::SendB,
            },
        ),
        ("track.mute", mute(1)),
        ("track 2 gain", gain(2)),
        ("track 2 filter.cutoff", cutoff(2)),
        ("filter.cutoff", cutoff(1)),
        (
            "filter.resonance",
            ParameterTarget::Synth {
                track: sel,
                param: SynthParamId::FilterResonance,
            },
        ),
        (
            "amp_release",
            ParameterTarget::Synth {
                track: sel,
                param: SynthParamId::AmpRelease,
            },
        ),
        (
            "sample.reverse",
            ParameterTarget::Sample {
                track: sel,
                param: SampleParamId::Reverse,
            },
        ),
        (
            "fx.master.0.4",
            ParameterTarget::Effect {
                location: EffectLocation::Master,
                slot: EffectSlotId(0),
                param: EffectParamId(4),
            },
        ),
        (
            "fx.track.1.2",
            ParameterTarget::Effect {
                location: EffectLocation::Track(sel),
                slot: EffectSlotId(1),
                param: EffectParamId(2),
            },
        ),
        (
            "fx.send_b.0.0",
            ParameterTarget::Effect {
                location: EffectLocation::Send(1),
                slot: EffectSlotId(0),
                param: EffectParamId(0),
            },
        ),
        (
            "transport.play",
            ParameterTarget::Action(ActionId::TogglePlay),
        ),
        (
            "transport.restart",
            ParameterTarget::Action(ActionId::Restart),
        ),
        ("panic", ParameterTarget::Action(ActionId::Panic)),
        ("scene.next", ParameterTarget::Action(ActionId::SceneNext)),
        ("scene.prev", ParameterTarget::Action(ActionId::ScenePrev)),
        (
            "scene.launch.7",
            ParameterTarget::Action(ActionId::SceneLaunch(7)),
        ),
    ];
    for (spec, expected) in cases {
        assert_eq!(parse_target(spec, sel), Ok(expected), "{spec}");
        let formatted = format_target(expected);
        assert_eq!(
            parse_target(&formatted, TrackId(99)),
            Ok(expected),
            "roundtrip of {spec} via {formatted}"
        );
    }
    for bad in [
        "",
        "nope",
        "track x gain",
        "fx.master.a.0",
        "fx.bogus.0.0",
        "scene.launch.",
        "oscillator",
        "sample.nope",
    ] {
        assert!(parse_target(bad, sel).is_err(), "{bad} should fail");
    }
}

#[test]
fn format_target_roundtrips_every_synth_param() {
    use SynthParamId as P;
    for param in [
        P::Octave,
        P::Semitone,
        P::FineCents,
        P::PulseWidth,
        P::AmpAttack,
        P::AmpDecay,
        P::AmpSustain,
        P::AmpRelease,
        P::FilterCutoff,
        P::FilterResonance,
        P::FilterKeytrack,
        P::FilterAttack,
        P::FilterDecay,
        P::FilterSustain,
        P::FilterRelease,
        P::FilterEnvAmount,
        P::OutputGain,
    ] {
        let target = ParameterTarget::Synth {
            track: TrackId(4),
            param,
        };
        assert_eq!(parse_target(&format_target(target), TrackId(0)), Ok(target));
    }
}

#[test]
fn parse_learn_commands() {
    let sel = TrackId(0);
    assert_eq!(
        parse_learn_command("learn track 2 filter.cutoff", sel),
        Ok(LearnCommand::Learn {
            target: cutoff(2),
            allow_notes: false
        })
    );
    assert_eq!(
        parse_learn_command("learn master.gain", sel),
        Ok(LearnCommand::Learn {
            target: ParameterTarget::Global(GlobalParamId::MasterGain),
            allow_notes: false
        })
    );
    assert_eq!(
        parse_learn_command("learn notes scene.next", sel),
        Ok(LearnCommand::Learn {
            target: ParameterTarget::Action(ActionId::SceneNext),
            allow_notes: true
        })
    );
    assert_eq!(
        parse_learn_command("learn cancel", sel),
        Ok(LearnCommand::Cancel)
    );
    assert_eq!(
        parse_learn_command("learn confirm", sel),
        Ok(LearnCommand::Confirm { policy: None })
    );
    assert_eq!(
        parse_learn_command("learn confirm replace", sel),
        Ok(LearnCommand::Confirm {
            policy: Some(ConflictPolicy::Replace)
        })
    );
    assert_eq!(
        parse_learn_command("learn confirm add", sel),
        Ok(LearnCommand::Confirm {
            policy: Some(ConflictPolicy::AddSecondary)
        })
    );
    assert_eq!(
        parse_learn_command("learn confirm cancel", sel),
        Ok(LearnCommand::Confirm {
            policy: Some(ConflictPolicy::Cancel)
        })
    );
    assert_eq!(
        parse_learn_command("unlearn 14", sel),
        Ok(LearnCommand::Unlearn(MappingId(14)))
    );
    assert_eq!(parse_learn_command("mappings", sel), Ok(LearnCommand::List));
    assert_eq!(
        parse_learn_command("mapping 14 range 200 8000", sel),
        Ok(LearnCommand::SetRange {
            id: MappingId(14),
            min: 200.0,
            max: 8000.0
        })
    );
    assert_eq!(
        parse_learn_command("mapping 14 invert on", sel),
        Ok(LearnCommand::SetInverted {
            id: MappingId(14),
            inverted: true
        })
    );
    assert_eq!(
        parse_learn_command("mapping 14 invert off", sel),
        Ok(LearnCommand::SetInverted {
            id: MappingId(14),
            inverted: false
        })
    );
    assert_eq!(
        parse_learn_command("mapping 14 pickup pickup", sel),
        Ok(LearnCommand::SetPickup {
            id: MappingId(14),
            mode: PickupMode::Pickup
        })
    );
    assert_eq!(
        parse_learn_command("mapping 14 pickup scale", sel),
        Ok(LearnCommand::SetPickup {
            id: MappingId(14),
            mode: PickupMode::Scale
        })
    );
    assert_eq!(
        parse_learn_command("mapping 14 button toggle", sel),
        Ok(LearnCommand::SetButton {
            id: MappingId(14),
            mode: ButtonMode::Toggle
        })
    );
    for bad in [
        "",
        "learn",
        "learn confirm maybe",
        "unlearn x",
        "mapping 14",
        "mapping 14 range 1",
        "mapping 14 range a b",
        "mapping 14 range nan 1",
        "mapping 14 invert maybe",
        "mapping 14 pickup slow",
        "mapping 14 button hold",
        "mappings extra",
        "play",
    ] {
        assert!(parse_learn_command(bad, sel).is_err(), "{bad} should fail");
    }
}
