use shelloop::{
    parse_pattern_edit_command, Pattern, PatternEditCommand, PatternStep, ProjectPatternEditors,
    QuantizeBoundary, StepEdit, TrackId,
};

fn pattern(name: &str, note: u8) -> Pattern {
    Pattern::new(
        name,
        u64::from(note),
        0.0,
        0,
        vec![
            Some(PatternStep {
                note,
                velocity: 0.8,
                gate: 0.5,
                probability: 1.0,
                ratchets: 1,
                microtiming_frames: 0,
            }),
            None,
            None,
            None,
        ],
    )
    .unwrap()
}

#[test]
fn command_parser_uses_human_one_based_step_numbers() {
    assert_eq!(
        parse_pattern_edit_command("track 2").unwrap(),
        PatternEditCommand::SelectTrack(TrackId(2))
    );
    assert_eq!(
        parse_pattern_edit_command("step 5 note 38").unwrap(),
        PatternEditCommand::Step {
            index: 4,
            edit: StepEdit::Note(38),
        }
    );
    assert_eq!(
        parse_pattern_edit_command("step 1 micro +120").unwrap(),
        PatternEditCommand::Step {
            index: 0,
            edit: StepEdit::Microtiming(120),
        }
    );
    assert_eq!(
        parse_pattern_edit_command("rotate right 3").unwrap(),
        PatternEditCommand::RotateRight(3)
    );
}

#[test]
fn command_parser_rejects_invalid_ranges_and_extra_arguments() {
    assert!(parse_pattern_edit_command("step 0 toggle").is_err());
    assert!(parse_pattern_edit_command("step 257 toggle").is_err());
    assert!(parse_pattern_edit_command("step 1 note 128").is_err());
    assert!(parse_pattern_edit_command("step 1 velocity 1.1").is_err());
    assert!(parse_pattern_edit_command("step 1 probability -0.1").is_err());
    assert!(parse_pattern_edit_command("step 1 ratchets 0").is_err());
    assert!(parse_pattern_edit_command("swing 0.8").is_err());
    assert!(parse_pattern_edit_command("undo now").is_err());
}

#[test]
fn project_editor_changes_only_the_selected_track() {
    let mut editors = ProjectPatternEditors::new(
        vec![
            (TrackId(1), pattern("one", 60)),
            (TrackId(2), pattern("two", 67)),
        ],
        8,
    )
    .unwrap();

    editors
        .apply(PatternEditCommand::SelectTrack(TrackId(2)))
        .unwrap();
    let result = editors
        .apply(PatternEditCommand::Step {
            index: 1,
            edit: StepEdit::Note(72),
        })
        .unwrap();

    assert_eq!(result.track, TrackId(2));
    assert_eq!(result.revision, Some(1));
    assert_eq!(editors.editor(TrackId(1)).unwrap().pattern().steps[1], None);
    assert_eq!(
        editors.editor(TrackId(2)).unwrap().pattern().steps[1]
            .unwrap()
            .note,
        72
    );
}

#[test]
fn field_edit_on_empty_step_creates_a_valid_default_step() {
    let mut editors =
        ProjectPatternEditors::new(vec![(TrackId(4), pattern("four", 48))], 8).unwrap();

    editors
        .apply(PatternEditCommand::Step {
            index: 2,
            edit: StepEdit::Probability(0.35),
        })
        .unwrap();

    let step = editors.editor(TrackId(4)).unwrap().pattern().steps[2].unwrap();
    assert_eq!(step.note, 60);
    assert_eq!(step.velocity, 0.8);
    assert_eq!(step.probability, 0.35);
    assert_eq!(step.ratchets, 1);
}

#[test]
fn undo_redo_history_is_isolated_per_track() {
    let mut editors = ProjectPatternEditors::new(
        vec![
            (TrackId(1), pattern("one", 60)),
            (TrackId(2), pattern("two", 67)),
        ],
        8,
    )
    .unwrap();

    editors
        .apply(PatternEditCommand::Step {
            index: 1,
            edit: StepEdit::Toggle,
        })
        .unwrap();
    editors
        .apply(PatternEditCommand::SelectTrack(TrackId(2)))
        .unwrap();
    editors.apply(PatternEditCommand::Undo).unwrap();

    assert!(editors.editor(TrackId(1)).unwrap().pattern().steps[1].is_some());
    assert_eq!(editors.editor(TrackId(2)).unwrap().pattern().steps[1], None);

    editors
        .apply(PatternEditCommand::SelectTrack(TrackId(1)))
        .unwrap();
    assert!(editors.apply(PatternEditCommand::Undo).unwrap().changed);
    assert_eq!(editors.editor(TrackId(1)).unwrap().pattern().steps[1], None);
    assert!(editors.apply(PatternEditCommand::Redo).unwrap().changed);
    assert!(editors.editor(TrackId(1)).unwrap().pattern().steps[1].is_some());
}

#[test]
fn selected_editor_compiles_directly_to_realtime_quantized_change() {
    let mut editors =
        ProjectPatternEditors::new(vec![(TrackId(9), pattern("nine", 60))], 8).unwrap();

    editors
        .apply(PatternEditCommand::Swing(0.2))
        .unwrap();
    let (track, queued) = editors
        .queue_selected_revision(1, 100, 60.0, 1, QuantizeBoundary::Step)
        .unwrap();

    assert_eq!(track, TrackId(9));
    assert_eq!(queued.apply_at_frame, 100);
    assert_eq!(queued.value.revision, 1);
    assert_eq!(queued.value.pattern.len(), 4);
}
