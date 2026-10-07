use shelloop::variation::{InvariantKind, InvariantProfile, InvariantRule};
use shelloop::{
    apply_lock_command, derive_seed, evaluate_invariants, format_locks, format_proposal,
    generate_variation, parse_pattern_json, parse_variation_command, pattern_hash, Pattern,
    PatternEditor, PatternStep, VariationCommand, VariationRequest, VariationSession,
    MAX_VARIATION_ATTEMPTS, VARIATION_ALGORITHM_VERSION,
};

fn step(note: u8) -> PatternStep {
    PatternStep {
        note,
        velocity: 0.8,
        gate: 0.5,
        probability: 1.0,
        ratchets: 1,
        microtiming_frames: 0,
    }
}

/// 16 steps, notes on odd one-based steps: 1,3,5,...,15.
fn fixture() -> Pattern {
    let notes = [48, 0, 55, 0, 60, 0, 52, 0, 48, 0, 62, 0, 57, 0, 50, 0];
    let steps = notes.iter().map(|&n| (n != 0).then(|| step(n))).collect();
    Pattern::new("fixture", 7, 0.08, 0, steps).unwrap()
}

fn with_rules(mut pattern: Pattern, rules: Vec<InvariantRule>) -> Pattern {
    pattern.invariants = Some(InvariantProfile::new(rules));
    pattern.validate().unwrap();
    pattern
}

fn rule(id: &str, kind: InvariantKind) -> InvariantRule {
    InvariantRule::new(id, kind)
}

fn profile(rules: Vec<InvariantRule>) -> InvariantProfile {
    InvariantProfile::new(rules)
}

fn eval_one(kind: InvariantKind, source: &Pattern, candidate: &Pattern) -> bool {
    let results = evaluate_invariants(&profile(vec![rule("r", kind)]), source, candidate);
    assert_eq!(results.len(), 1);
    results[0].passed
}

// --- schema / persistence -------------------------------------------------

#[test]
fn legacy_pattern_json_without_invariants_loads_and_roundtrips() {
    let legacy = r#"{"name":"Legacy","seed":3,"swing":0.0,"channel":0,
        "steps":[{"note":60,"velocity":0.8,"gate":0.5,"probability":1.0,"ratchets":1,"microtiming_frames":0},null]}"#;
    let pattern = parse_pattern_json(legacy).unwrap();
    assert!(pattern.invariants.is_none());
    let json = serde_json::to_string(&pattern).unwrap();
    assert!(!json.contains("invariants"));
    assert_eq!(parse_pattern_json(&json).unwrap(), pattern);
}

#[test]
fn spec_example_profile_parses_and_roundtrips() {
    let json = r#"{
        "name": "Example", "seed": 42, "swing": 0.08, "channel": 1,
        "steps": [null,null,null,null,null,null,null,null,null,null,null,null,null,null,null,null],
        "invariants": {
          "version": 1,
          "rules": [
            {"id": "hook", "kind": "anchor_steps", "enabled": true, "steps": [1, 5, 9, 13]},
            {"id": "rhythm", "kind": "activity_mask", "enabled": true},
            {"id": "contour", "kind": "pitch_contour", "enabled": false}
          ]
        }
    }"#;
    let pattern = parse_pattern_json(json).unwrap();
    let profile = pattern.invariants.clone().unwrap();
    assert_eq!(profile.rules.len(), 3);
    assert_eq!(
        profile.rules[0].kind,
        InvariantKind::AnchorSteps {
            steps: vec![1, 5, 9, 13]
        }
    );
    assert_eq!(profile.rules[1].kind, InvariantKind::ActivityMask);
    assert!(!profile.rules[2].enabled);
    let roundtrip = parse_pattern_json(&serde_json::to_string(&pattern).unwrap()).unwrap();
    assert_eq!(roundtrip, pattern);
    // enabled defaults to true; note_range params roundtrip
    let rule: InvariantRule =
        serde_json::from_str(r#"{"id":"r","kind":"note_range","low":24,"high":60}"#).unwrap();
    assert!(rule.enabled);
    assert_eq!(rule.kind, InvariantKind::NoteRange { low: 24, high: 60 });
}

fn pattern_json_with_invariants(invariants: &str) -> String {
    format!(
        r#"{{"name":"P","seed":1,"swing":0.0,"channel":0,"steps":[null,null,null,null],"invariants":{invariants}}}"#
    )
}

#[test]
fn malformed_profiles_are_rejected_on_load() {
    let cases = [
        (r#"{"version":2,"rules":[]}"#, "version"),
        (
            r#"{"version":1,"rules":[{"id":"a","kind":"activity_mask"},{"id":"a","kind":"event_count"}]}"#,
            "duplicate",
        ),
        (
            r#"{"version":1,"rules":[{"id":"x","kind":"groove_lock"}]}"#,
            "unknown invariant kind",
        ),
        (
            r#"{"version":1,"rules":[{"id":"","kind":"event_count"}]}"#,
            "empty id",
        ),
        (
            r#"{"version":1,"rules":[{"id":"a","kind":"anchor_steps","steps":[0]}]}"#,
            "anchor step 0",
        ),
        (
            r#"{"version":1,"rules":[{"id":"a","kind":"anchor_steps","steps":[5]}]}"#,
            "anchor step 5",
        ),
        (
            r#"{"version":1,"rules":[{"id":"a","kind":"anchor_steps","steps":[]}]}"#,
            "at least one",
        ),
        (
            r#"{"version":1,"rules":[{"id":"a","kind":"anchor_steps","steps":[2,2]}]}"#,
            "twice",
        ),
        (
            r#"{"version":1,"rules":[{"id":"a","kind":"anchor_steps"}]}"#,
            "requires 'steps'",
        ),
        (
            r#"{"version":1,"rules":[{"id":"n","kind":"note_range","low":70,"high":60}]}"#,
            "low <= high",
        ),
        (
            r#"{"version":1,"rules":[{"id":"n","kind":"note_range","low":0,"high":128}]}"#,
            "low <= high",
        ),
        (
            r#"{"version":1,"rules":[{"id":"n","kind":"note_range","low":0}]}"#,
            "requires 'low'",
        ),
        (
            r#"{"version":1,"rules":[{"id":"n","kind":"event_count","steps":[1]}]}"#,
            "does not take",
        ),
        (
            r#"{"version":1,"rules":[{"id":"n","kind":"event_count","bogus":1}]}"#,
            "unknown field",
        ),
        (r#"{"version":1,"rules":[],"extra":true}"#, "unknown field"),
        (
            r#"{"version":1,"rules":[{"id":"a","kind":"note_range","low":0,"high":10},{"id":"b","kind":"note_range","low":20,"high":30}]}"#,
            "contradictory invariants 'a' and 'b'",
        ),
    ];
    for (profile, expected) in cases {
        let error = parse_pattern_json(&pattern_json_with_invariants(profile)).unwrap_err();
        assert!(
            error.contains(expected),
            "profile {profile}: error '{error}' lacks '{expected}'"
        );
    }
    // Disabled conflicting ranges are fine.
    parse_pattern_json(&pattern_json_with_invariants(
        r#"{"version":1,"rules":[{"id":"a","kind":"note_range","low":0,"high":10},{"id":"b","kind":"note_range","enabled":false,"low":20,"high":30}]}"#,
    ))
    .unwrap();
}

#[test]
fn fuzzed_numeric_boundaries_never_panic() {
    for steps in ["[1]", "[4]", "[65535]", "[1,2,3,4]", "[-1]", "[70000]"] {
        let json = pattern_json_with_invariants(&format!(
            r#"{{"version":1,"rules":[{{"id":"a","kind":"anchor_steps","steps":{steps}}}]}}"#
        ));
        let result = parse_pattern_json(&json);
        assert_eq!(
            result.is_ok(),
            steps == "[1]" || steps == "[4]" || steps == "[1,2,3,4]"
        );
    }
    for (low, high) in [(0, 0), (127, 127), (0, 127), (128, 128), (-1, 3), (5, 4)] {
        let json = pattern_json_with_invariants(&format!(
            r#"{{"version":1,"rules":[{{"id":"n","kind":"note_range","low":{low},"high":{high}}}]}}"#
        ));
        let ok = (0..=127).contains(&low) && (0..=127).contains(&high) && low <= high;
        assert_eq!(parse_pattern_json(&json).is_ok(), ok, "{low}..{high}");
    }
}

// --- evaluation -----------------------------------------------------------

#[test]
fn anchor_steps_compare_full_values_including_absence() {
    let source = fixture();
    let anchors = InvariantKind::AnchorSteps {
        steps: vec![1, 2, 5],
    };
    let mut ok = source.clone();
    ok.steps[2] = Some(step(70)); // step 3 not anchored
    assert!(eval_one(anchors.clone(), &source, &ok));

    let mut velocity = source.clone();
    velocity.steps[4].as_mut().unwrap().velocity = 0.81;
    assert!(!eval_one(anchors.clone(), &source, &velocity));

    let mut filled_rest = source.clone();
    filled_rest.steps[1] = Some(step(60)); // step 2 anchored rest
    assert!(!eval_one(anchors, &source, &filled_rest));
}

#[test]
fn activity_mask_allows_value_changes_only() {
    let source = fixture();
    let mut ok = source.clone();
    ok.steps[0].as_mut().unwrap().note = 30;
    ok.steps[2].as_mut().unwrap().ratchets = 4;
    assert!(eval_one(InvariantKind::ActivityMask, &source, &ok));
    let mut bad = source.clone();
    bad.steps[1] = Some(step(60));
    assert!(!eval_one(InvariantKind::ActivityMask, &source, &bad));
    let mut shorter = source.clone();
    shorter.steps.pop();
    assert!(!eval_one(InvariantKind::ActivityMask, &source, &shorter));
}

#[test]
fn pitch_contour_preserves_intervals_and_positions() {
    let source = fixture();
    let mut transposed = source.clone();
    for s in transposed.steps.iter_mut().flatten() {
        s.note += 5;
        s.velocity = 0.3;
    }
    assert!(eval_one(InvariantKind::PitchContour, &source, &transposed));

    let mut interval = source.clone();
    interval.steps[4].as_mut().unwrap().note = 61;
    assert!(!eval_one(InvariantKind::PitchContour, &source, &interval));

    // Same note sequence shifted to different positions: intervals equal but
    // event/rest positions differ.
    let mut moved = source.clone();
    moved.steps[1] = moved.steps[0].take();
    assert!(!eval_one(InvariantKind::PitchContour, &source, &moved));
}

#[test]
fn event_count_allows_moving_events() {
    let source = fixture();
    let mut moved = source.clone();
    moved.steps[1] = moved.steps[0].take();
    assert!(eval_one(InvariantKind::EventCount, &source, &moved));
    let mut extra = source.clone();
    extra.steps[1] = Some(step(60));
    assert!(!eval_one(InvariantKind::EventCount, &source, &extra));
}

#[test]
fn note_range_is_a_constraint_on_candidate_notes() {
    let source = fixture();
    let range = InvariantKind::NoteRange { low: 48, high: 62 };
    assert!(eval_one(range.clone(), &source, &source));
    let mut high = source.clone();
    high.steps[0].as_mut().unwrap().note = 63;
    assert!(!eval_one(range.clone(), &source, &high));
    let mut rest = source.clone();
    rest.steps[0] = None; // rests are unconstrained
    assert!(eval_one(range, &source, &rest));
}

#[test]
fn combined_rules_evaluate_in_sorted_id_order_with_diagnostics() {
    let source = fixture();
    let p = InvariantProfile::new(vec![
        rule("zeta", InvariantKind::EventCount),
        rule("alpha", InvariantKind::NoteRange { low: 40, high: 70 }),
        InvariantRule {
            id: "disabled".into(),
            enabled: false,
            kind: InvariantKind::ActivityMask,
        },
        rule("mid", InvariantKind::ActivityMask),
    ]);
    let results = evaluate_invariants(&p, &source, &source);
    let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
    assert_eq!(ids, ["alpha", "mid", "zeta"]);
    assert!(results.iter().all(|r| r.passed));

    let mut candidate = source.clone();
    candidate.steps[1] = Some(step(90));
    let results = evaluate_invariants(&p, &source, &candidate);
    assert!(results.iter().all(|r| !r.passed));
    assert!(results[0].detail.contains("step 2 note 90"));
    assert_eq!(results, evaluate_invariants(&p, &source, &candidate));
}

// --- generator ------------------------------------------------------------

fn request(seed: u64, amount: f32) -> VariationRequest {
    VariationRequest { seed, amount }
}

#[test]
fn identical_inputs_yield_identical_proposals() {
    let source = with_rules(
        fixture(),
        vec![rule(
            "hook",
            InvariantKind::AnchorSteps { steps: vec![1, 9] },
        )],
    );
    let a = generate_variation(&source, request(821, 0.5)).unwrap();
    let b = generate_variation(&source, request(821, 0.5)).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.algorithm_version, VARIATION_ALGORITHM_VERSION);
    assert_eq!(a.source_hash, pattern_hash(&source));
    assert_eq!(a.candidate_hash, pattern_hash(&a.candidate));
    assert_ne!(a.source_hash, a.candidate_hash);
    assert!(!a.changes.is_empty());
    let c = generate_variation(&source, request(822, 0.5)).unwrap();
    assert_ne!(a.candidate_hash, c.candidate_hash);
}

/// Pinned cross-platform fixture: any change to the PRNG, operation
/// selection, float handling or hash encoding changes these values and must
/// come with a VARIATION_ALGORITHM_VERSION bump.
#[test]
fn fixed_fixture_hashes_are_stable_across_platforms() {
    let source = fixture();
    assert_eq!(pattern_hash(&source), 0x2fcb_fea1_205f_4256);
    let proposal = generate_variation(&source, request(821, 0.25)).unwrap();
    assert_eq!(proposal.candidate_hash, 0xe1da_f47c_491d_4262);
    assert_eq!(proposal.attempts, 1);
    let changes: Vec<(u16, &str, &str, &str)> = proposal
        .changes
        .iter()
        .map(|c| (c.step, c.field, c.before.as_str(), c.after.as_str()))
        .collect();
    let rest = "rest";
    assert_eq!(
        changes,
        [
            (3, "velocity", "0.800", "0.900"),
            (
                5,
                "active",
                "note 60 vel 0.800 gate 0.500 prob 1.000 ratchets 1 micro 0",
                rest
            ),
            (9, "note", "48", "57"),
            (
                10,
                "active",
                rest,
                "note 48 vel 0.800 gate 0.500 prob 1.000 ratchets 1 micro 0"
            ),
        ]
    );
    assert_eq!(derive_seed(42, 3, 1), 0xabeb_b975_bd6c_dd3d);
}

#[test]
fn amount_zero_changes_nothing() {
    let source = with_rules(fixture(), vec![rule("count", InvariantKind::EventCount)]);
    let proposal = generate_variation(&source, request(1, 0.0)).unwrap();
    assert!(proposal.changes.is_empty());
    assert_eq!(proposal.attempts, 0);
    assert_eq!(proposal.candidate, source);
    assert_eq!(proposal.candidate_hash, proposal.source_hash);
    assert!(proposal.invariant_results.iter().all(|r| r.passed));
}

#[test]
fn invalid_amounts_are_rejected() {
    for amount in [-0.1, 1.01, f32::NAN, f32::INFINITY] {
        let failure = generate_variation(&fixture(), request(1, amount)).unwrap_err();
        assert_eq!(failure.attempts, 0);
        assert!(failure.message.contains("amount"));
    }
}

#[test]
fn amount_one_is_bounded_and_obeys_combined_locks() {
    let source = with_rules(
        fixture(),
        vec![
            rule(
                "anchors",
                InvariantKind::AnchorSteps {
                    steps: vec![1, 2, 5],
                },
            ),
            rule("rhythm", InvariantKind::ActivityMask),
            rule("range", InvariantKind::NoteRange { low: 45, high: 65 }),
        ],
    );
    for seed in 0..64 {
        let proposal = generate_variation(&source, request(seed, 1.0)).unwrap();
        let candidate = &proposal.candidate;
        candidate.validate().unwrap();
        assert!(proposal.attempts <= MAX_VARIATION_ATTEMPTS);
        assert!(proposal.invariant_results.iter().all(|r| r.passed));
        assert_eq!(proposal.invariant_results.len(), 3);
        assert_eq!(candidate.steps[0], source.steps[0]);
        assert_eq!(candidate.steps[1], source.steps[1]);
        assert_eq!(candidate.steps[4], source.steps[4]);
        assert_eq!(candidate.invariants, source.invariants);
        assert_eq!(candidate.locks, source.locks);
        assert!((0.0..=0.75).contains(&candidate.swing));
        for (before, after) in source.steps.iter().zip(&candidate.steps) {
            assert_eq!(before.is_some(), after.is_some());
            if let (Some(b), Some(a)) = (before, after) {
                assert!((45..=65).contains(&a.note));
                assert!(i16::from(a.note).abs_diff(i16::from(b.note)) <= 12);
                assert!((1..=8).contains(&a.ratchets));
                assert!(a.microtiming_frames.abs() <= 240);
                for v in [a.velocity, a.gate, a.probability] {
                    assert!((0.0..=1.0).contains(&v));
                }
            }
        }
        for change in &proposal.changes {
            assert!(change.step as usize <= source.steps.len());
            assert!(![1, 2, 5].contains(&change.step));
        }
    }
}

#[test]
fn unlocked_variation_may_toggle_and_new_notes_come_from_neighbours() {
    let source = fixture();
    let mut toggled_on = false;
    for seed in 0..64 {
        let proposal = generate_variation(&source, request(seed, 1.0)).unwrap();
        for change in proposal.changes.iter().filter(|c| c.field == "active") {
            let index = usize::from(change.step - 1);
            if source.steps[index].is_none() {
                toggled_on = true;
                let new = proposal.candidate.steps[index].unwrap();
                let neighbour = source.steps[index - 1].unwrap();
                assert_eq!(new.note, neighbour.note);
                assert_eq!(new.ratchets, 1);
            }
        }
    }
    assert!(toggled_on);
}

#[test]
fn contour_and_count_locks_hold_under_variation() {
    let source = with_rules(
        fixture(),
        vec![
            rule("contour", InvariantKind::PitchContour),
            rule("count", InvariantKind::EventCount),
        ],
    );
    for seed in 0..32 {
        let proposal = generate_variation(&source, request(seed, 0.75)).unwrap();
        assert!(proposal.invariant_results.iter().all(|r| r.passed));
        let notes = |p: &Pattern| -> Vec<Option<u8>> {
            p.steps.iter().map(|s| s.map(|s| s.note)).collect()
        };
        assert_eq!(notes(&proposal.candidate), notes(&source));
    }
}

#[test]
fn impossible_profile_terminates_at_cap_and_reports_rejecting_locks() {
    let mut source = fixture();
    for s in source.steps.iter_mut().flatten() {
        s.note = 100;
    }
    let source = with_rules(
        source,
        vec![
            rule("rhythm", InvariantKind::ActivityMask),
            rule("low", InvariantKind::NoteRange { low: 0, high: 10 }),
        ],
    );
    let failure = generate_variation(&source, request(5, 0.25)).unwrap_err();
    assert_eq!(failure.attempts, MAX_VARIATION_ATTEMPTS);
    assert!(failure
        .rejected_by
        .iter()
        .any(|(id, n)| id == "low" && *n > 0));
    assert!(failure.message.contains("reducing --amount"));
    assert_eq!(
        failure,
        generate_variation(&source, request(5, 0.25)).unwrap_err()
    );
}

#[test]
fn contradictory_profiles_fail_before_attempts_with_conflicting_ids() {
    let anchored = with_rules(
        fixture(),
        vec![
            rule("hook", InvariantKind::AnchorSteps { steps: vec![1] }),
            rule("range", InvariantKind::NoteRange { low: 50, high: 70 }),
        ],
    );
    let failure = generate_variation(&anchored, request(1, 0.5)).unwrap_err();
    assert_eq!(failure.attempts, 0);
    assert_eq!(failure.conflicting_ids, ["hook", "range"]);

    let contour = with_rules(
        fixture(),
        vec![
            rule("contour", InvariantKind::PitchContour),
            rule("range", InvariantKind::NoteRange { low: 50, high: 55 }),
        ],
    );
    let failure = generate_variation(&contour, request(1, 0.5)).unwrap_err();
    assert_eq!(failure.conflicting_ids, ["contour", "range"]);
}

#[test]
fn derive_seed_is_deterministic_and_varies() {
    assert_eq!(derive_seed(1, 2, 3), derive_seed(1, 2, 3));
    assert_ne!(derive_seed(1, 2, 3), derive_seed(1, 2, 4));
    assert_ne!(derive_seed(1, 2, 3), derive_seed(1, 3, 3));
    assert_ne!(derive_seed(1, 2, 3), derive_seed(2, 2, 3));
}

// --- lifecycle ------------------------------------------------------------

fn editor() -> PatternEditor {
    PatternEditor::new(fixture(), 16).unwrap()
}

#[test]
fn preview_does_not_touch_editor() {
    let editor = editor();
    let before = editor.pattern().clone();
    let mut session = VariationSession::new();
    let proposal = session.preview(&editor, request(9, 0.5)).unwrap();
    assert_eq!(proposal.source_revision, Some(0));
    assert_eq!(editor.pattern(), &before);
    assert_eq!(editor.revision(), 0);
    assert_eq!(editor.undo_depth(), 0);
    assert_eq!(editor.redo_depth(), 0);
    assert!(session.proposal().is_some());
}

#[test]
fn new_preview_replaces_and_failed_preview_keeps_prior() {
    let editor = editor();
    let mut session = VariationSession::new();
    let first = session.preview(&editor, request(1, 0.5)).unwrap().clone();
    let second = session.preview(&editor, request(2, 0.5)).unwrap().clone();
    assert_ne!(first, second);
    assert!(session.preview(&editor, request(3, 2.0)).is_err());
    assert_eq!(session.proposal(), Some(&second));
}

#[test]
fn stale_preview_is_rejected_after_an_edit() {
    let mut editor = editor();
    let mut session = VariationSession::new();
    session.preview(&editor, request(4, 0.5)).unwrap();
    editor.set_swing(0.1).unwrap();
    let error = session.accept(&mut editor).unwrap_err();
    assert!(error.contains("stale"));
    assert!(session.proposal().is_none());
    assert_eq!(editor.undo_depth(), 1);
}

#[test]
fn accept_is_one_undo_step_and_undo_restores_pattern_and_invariants() {
    let mut editor = editor();
    let locks = apply_lock_command(None, &VariationCommand::LockRhythm(true)).unwrap();
    editor.set_invariants(locks.clone()).unwrap();
    let before = editor.pattern().clone();
    assert_eq!(before.invariants, locks);

    let mut session = VariationSession::new();
    let candidate = session
        .preview(&editor, request(11, 0.5))
        .unwrap()
        .candidate
        .clone();
    let depth = editor.undo_depth();
    editor.redo(); // no-op; redo empty
    let revision = session.accept(&mut editor).unwrap();
    assert_eq!(revision, editor.revision());
    assert_eq!(editor.pattern(), &candidate);
    assert_eq!(editor.undo_depth(), depth + 1);
    assert!(session.proposal().is_none());

    assert!(editor.undo());
    assert_eq!(editor.pattern(), &before);
    assert!(editor.undo());
    assert!(editor.pattern().invariants.is_none());
    assert!(editor.redo());
    assert!(editor.redo());
    assert_eq!(editor.pattern(), &candidate);
}

#[test]
fn reject_and_failed_accept_preserve_history() {
    let mut editor = editor();
    let mut session = VariationSession::new();
    session.preview(&editor, request(4, 0.5)).unwrap();
    assert!(session.reject());
    assert!(!session.reject());
    assert!(session.accept(&mut editor).is_err());
    assert_eq!(editor.undo_depth(), 0);
    assert_eq!(editor.revision(), 0);

    session.preview(&editor, request(4, 0.0)).unwrap();
    let error = session.accept(&mut editor).unwrap_err();
    assert!(error.contains("no changes"));
    assert_eq!(editor.undo_depth(), 0);
}

#[test]
fn impossible_preview_leaves_editor_untouched() {
    let mut pattern = fixture();
    for s in pattern.steps.iter_mut().flatten() {
        s.note = 100;
    }
    let mut editor = PatternEditor::new(pattern, 8).unwrap();
    editor
        .set_invariants(Some(profile(vec![
            rule("rhythm", InvariantKind::ActivityMask),
            rule("low", InvariantKind::NoteRange { low: 0, high: 10 }),
        ])))
        .unwrap();
    let before = editor.pattern().clone();
    let mut session = VariationSession::new();
    assert!(session.preview(&editor, request(1, 1.0)).is_err());
    assert!(session.proposal().is_none());
    assert_eq!(editor.pattern(), &before);
    assert_eq!(editor.undo_depth(), 1);
    assert_eq!(editor.revision(), 1);
}

#[test]
fn invalid_invariants_are_refused_by_editor() {
    let mut editor = editor();
    let bad = profile(vec![rule(
        "a",
        InvariantKind::AnchorSteps { steps: vec![17] },
    )]);
    assert!(editor.set_invariants(Some(bad)).is_err());
    assert_eq!(editor.undo_depth(), 0);
    let mut invalid = fixture();
    invalid.swing = 2.0;
    assert!(editor.replace_pattern(invalid).is_err());
    assert_eq!(editor.revision(), 0);
}

#[test]
fn session_seed_derivation_uses_monotonic_counter() {
    let mut session = VariationSession::new();
    let a = session.next_derived_seed(42, 3);
    let b = session.next_derived_seed(42, 3);
    assert_eq!(a, derive_seed(42, 3, 1));
    assert_eq!(b, derive_seed(42, 3, 2));
}

// --- commands -------------------------------------------------------------

#[test]
fn parses_variation_commands() {
    let cases = [
        (
            "variation lock anchors 1,5,9,13",
            VariationCommand::LockAnchors(vec![1, 5, 9, 13]),
        ),
        (
            "variation lock anchors 9, 1,9",
            VariationCommand::LockAnchors(vec![1, 9]),
        ),
        (
            "variation lock anchors off",
            VariationCommand::LockAnchors(vec![]),
        ),
        (
            "variation lock rhythm on",
            VariationCommand::LockRhythm(true),
        ),
        (
            "variation lock rhythm off",
            VariationCommand::LockRhythm(false),
        ),
        (
            "variation lock contour on",
            VariationCommand::LockContour(true),
        ),
        (
            "variation lock count off",
            VariationCommand::LockEventCount(false),
        ),
        (
            "variation lock note-range 24 60",
            VariationCommand::LockNoteRange(Some((24, 60))),
        ),
        (
            "variation lock note-range off",
            VariationCommand::LockNoteRange(None),
        ),
        ("variation locks", VariationCommand::ShowLocks),
        (
            "variation preview --seed 821 --amount 0.25",
            VariationCommand::Preview {
                seed: Some(821),
                amount: 0.25,
            },
        ),
        (
            "variation preview",
            VariationCommand::Preview {
                seed: None,
                amount: 0.25,
            },
        ),
        (
            "variation preview --amount 1",
            VariationCommand::Preview {
                seed: None,
                amount: 1.0,
            },
        ),
        ("variation accept", VariationCommand::Accept),
        ("variation reject", VariationCommand::Reject),
    ];
    for (line, expected) in cases {
        assert_eq!(parse_variation_command(line).unwrap(), expected, "{line}");
    }
}

#[test]
fn rejects_malformed_variation_commands() {
    for line in [
        "",
        "variation",
        "variations locks",
        "variation lock anchors",
        "variation lock anchors 0",
        "variation lock anchors 257",
        "variation lock anchors 1,x",
        "variation lock rhythm",
        "variation lock rhythm maybe",
        "variation lock note-range 60 24",
        "variation lock note-range 0 128",
        "variation lock note-range 24",
        "variation lock groove on",
        "variation preview --amount 1.5",
        "variation preview --amount NaN",
        "variation preview --seed -1",
        "variation preview --seed",
        "variation preview --seed 1 --seed 2",
        "variation preview extra",
        "variation accept now",
    ] {
        assert!(parse_variation_command(line).is_err(), "{line}");
    }
}

#[test]
fn lock_commands_maintain_stable_rule_ids() {
    let p = apply_lock_command(None, &VariationCommand::LockAnchors(vec![1, 5])).unwrap();
    let p = apply_lock_command(p, &VariationCommand::LockRhythm(true)).unwrap();
    let p = apply_lock_command(p, &VariationCommand::LockContour(true)).unwrap();
    let p = apply_lock_command(p, &VariationCommand::LockEventCount(true)).unwrap();
    let p = apply_lock_command(p, &VariationCommand::LockNoteRange(Some((24, 60)))).unwrap();
    let ids: Vec<&str> = p
        .as_ref()
        .unwrap()
        .rules
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    assert_eq!(ids, ["anchors", "rhythm", "contour", "count", "note_range"]);
    p.as_ref().unwrap().validate(16).unwrap();

    let p = apply_lock_command(p, &VariationCommand::LockAnchors(vec![2])).unwrap();
    assert_eq!(
        p.as_ref().unwrap().rule("anchors").unwrap().kind,
        InvariantKind::AnchorSteps { steps: vec![2] }
    );
    let lines = format_locks(p.as_ref());
    assert!(lines
        .iter()
        .any(|l| l.contains("anchors [on] anchor_steps steps 2")));
    assert!(lines
        .iter()
        .any(|l| l.contains("note_range [on] note_range 24..=60")));

    let mut p = p;
    for command in [
        VariationCommand::LockAnchors(vec![]),
        VariationCommand::LockRhythm(false),
        VariationCommand::LockContour(false),
        VariationCommand::LockEventCount(false),
    ] {
        p = apply_lock_command(p, &command).unwrap();
        assert!(p.is_some());
    }
    let p = apply_lock_command(p, &VariationCommand::LockNoteRange(None)).unwrap();
    assert!(p.is_none());
    assert_eq!(format_locks(None), ["variation locks: none"]);
    assert!(apply_lock_command(None, &VariationCommand::Accept).is_err());
    assert!(apply_lock_command(None, &VariationCommand::LockNoteRange(Some((9, 3)))).is_err());
}

#[test]
fn format_proposal_is_labelled_text_preview() {
    let source = with_rules(fixture(), vec![rule("rhythm", InvariantKind::ActivityMask)]);
    let proposal = generate_variation(&source, request(821, 0.5)).unwrap();
    let lines = format_proposal(&proposal);
    assert!(lines[0].contains("text preview"));
    assert!(lines.iter().any(|l| l.contains("seed 821")));
    assert!(lines
        .iter()
        .any(|l| l.contains(&format!("{:016x}", proposal.candidate_hash))));
    assert!(lines.iter().any(|l| l.contains("[pass] rhythm")));
    assert_eq!(
        lines
            .iter()
            .filter(|l| l.starts_with("  ") && l.contains("->"))
            .count(),
        proposal.changes.len()
    );
}
