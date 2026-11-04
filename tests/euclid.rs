//! Spec 16 acceptance: Euclidean rhythm command on the pattern editor.

use shelloop::{
    parse_pattern_edit_command, Pattern, PatternEditCommand, PatternStep, ProjectPatternEditors,
    QuantizeBoundary, TrackId,
};

fn step(note: u8) -> PatternStep {
    PatternStep {
        note,
        velocity: 0.7,
        gate: 0.4,
        probability: 1.0,
        ratchets: 1,
        microtiming_frames: 0,
    }
}

fn editors() -> ProjectPatternEditors {
    // 8 steps, hits on steps 1 and 5 with distinct notes.
    let mut steps = vec![None; 8];
    steps[0] = Some(step(36));
    steps[4] = Some(step(40));
    let pattern = Pattern::new("p", 1, 0.0, 0, steps).unwrap();
    ProjectPatternEditors::new(vec![(TrackId(1), pattern)], 8).unwrap()
}

fn hits(editors: &ProjectPatternEditors) -> Vec<usize> {
    let pattern = editors.editor(TrackId(1)).unwrap().pattern();
    pattern
        .steps
        .iter()
        .enumerate()
        .filter_map(|(index, step)| step.map(|_| index))
        .collect()
}

#[test]
fn euclid_parses_hits_and_optional_rotation() {
    assert_eq!(
        parse_pattern_edit_command("euclid 3").unwrap(),
        PatternEditCommand::Euclid {
            hits: 3,
            rotation: 0
        }
    );
    assert_eq!(
        parse_pattern_edit_command("euclid 5 2").unwrap(),
        PatternEditCommand::Euclid {
            hits: 5,
            rotation: 2
        }
    );
    assert!(parse_pattern_edit_command("euclid").is_err());
    assert!(parse_pattern_edit_command("euclid -1").is_err());
    assert!(parse_pattern_edit_command("euclid 3 1 9").is_err());
    assert!(parse_pattern_edit_command("euclid three").is_err());
}

#[test]
fn euclid_keeps_kept_hits_and_fills_new_ones_with_the_default_step() {
    let mut editors = editors();
    let outcome = editors
        .apply(PatternEditCommand::Euclid {
            hits: 3,
            rotation: 0,
        })
        .unwrap();
    assert!(outcome.changed);
    // E(3,8) = x..x..x. : steps 1, 4 and 7 (zero-based 0, 3, 6).
    assert_eq!(hits(&editors), vec![0, 3, 6]);
    let pattern = editors.editor(TrackId(1)).unwrap().pattern();
    assert_eq!(
        pattern.steps[0].unwrap().note,
        36,
        "kept hit keeps its note"
    );
    assert_eq!(
        pattern.steps[3].unwrap().note,
        60,
        "new hit uses default step"
    );
    assert_eq!(pattern.steps[6].unwrap().note, 60);
}

#[test]
fn euclid_is_one_undoable_quantised_transaction() {
    let mut editors = editors();
    let before = editors.editor(TrackId(1)).unwrap().pattern().clone();
    editors
        .apply(PatternEditCommand::Euclid {
            hits: 5,
            rotation: 1,
        })
        .unwrap();
    assert_eq!(
        editors.default_quantize_boundary(&PatternEditCommand::Euclid {
            hits: 5,
            rotation: 1
        }),
        QuantizeBoundary::Pattern { steps: 8 }
    );
    assert_eq!(hits(&editors).len(), 5);
    editors.apply(PatternEditCommand::Undo).unwrap();
    assert_eq!(editors.editor(TrackId(1)).unwrap().pattern(), &before);
}

#[test]
fn euclid_rejects_too_many_hits_without_changing_the_pattern() {
    let mut editors = editors();
    let before = editors.editor(TrackId(1)).unwrap().pattern().clone();
    assert!(editors
        .apply(PatternEditCommand::Euclid {
            hits: 9,
            rotation: 0
        })
        .is_err());
    assert_eq!(editors.editor(TrackId(1)).unwrap().pattern(), &before);
    let zero = editors
        .apply(PatternEditCommand::Euclid {
            hits: 0,
            rotation: 0,
        })
        .unwrap();
    assert!(zero.changed);
    assert!(hits(&editors).is_empty());
}

#[test]
fn controller_queues_euclid_as_a_revision_and_rejects_bad_hits() {
    use crossbeam_channel::bounded;
    use shelloop::{
        apply_audio_message, MultiTrackEngine, MultiTrackProject, SessionController,
        TrackDefinition, TrackKind,
    };

    let pattern = {
        let mut steps = vec![None; 8];
        steps[0] = Some(step(36));
        Pattern::new("p", 1, 0.0, 0, steps).unwrap()
    };
    let track = TrackDefinition::new(TrackId(1), "bass", TrackKind::Synth, pattern);
    let project = MultiTrackProject::new(9, 120.0, 4, vec![track]);
    let mut engine = MultiTrackEngine::from_project(48_000, 4, &project.clone(), None).unwrap();
    let (sender, receiver) = bounded(8);
    let mut controller = SessionController::new(project, None, 48_000, sender).unwrap();
    let (garbage, _) = bounded(4);
    macro_rules! run {
        ($line:expr) => {{
            let result = controller.execute($line, engine.position_frame());
            while let Ok(message) = receiver.try_recv() {
                apply_audio_message(&mut engine, message, &garbage);
            }
            result
        }};
    }
    assert_eq!(engine.track_active_revision(TrackId(1)), Some(0));
    assert!(run!("euclid 4").is_ok());
    // A boundary edit queued at frame 0 applies at the first frame.
    for _ in 0..4 {
        engine.next_stereo_frame();
    }
    assert_eq!(engine.track_active_revision(TrackId(1)), Some(1));
    assert!(run!("euclid 9").is_err());
    assert!(run!("euclid 2 0 0").is_err());
    assert_eq!(engine.track_active_revision(TrackId(1)), Some(1));
}
