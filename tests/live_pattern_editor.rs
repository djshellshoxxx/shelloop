use shelloop::{Pattern, PatternEditor, PatternStep, QuantizeBoundary};

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

fn base_pattern() -> Pattern {
    Pattern::new(
        "edit me",
        77,
        0.0,
        0,
        vec![Some(step(60)), None, Some(step(64)), None],
    )
    .unwrap()
}

#[test]
fn live_editor_never_mutates_the_original_active_pattern() {
    let active = base_pattern();
    let mut editor = PatternEditor::new(active.clone(), 8).unwrap();

    editor.toggle_step(1, step(62)).unwrap();
    assert_eq!(active.steps[1], None);
    assert_eq!(editor.pattern().steps[1], Some(step(62)));
    assert_eq!(editor.revision(), 1);
}

#[test]
fn editor_queues_a_validated_revision_on_existing_quantized_clock() {
    let mut editor = PatternEditor::new(base_pattern(), 8).unwrap();
    editor.set_step(1, Some(step(67))).unwrap();

    let queued = editor
        .queue_revision(100, 48_000, 120.0, 4, QuantizeBoundary::Step)
        .unwrap();

    assert_eq!(queued.apply_at_frame, 6_000);
    assert_eq!(queued.value.revision, 1);
    assert_eq!(queued.value.pattern.steps[1], Some(step(67)));
}

#[test]
fn invalid_edit_is_atomic_and_does_not_advance_revision() {
    let mut editor = PatternEditor::new(base_pattern(), 8).unwrap();
    let before = editor.pattern().clone();

    let mut invalid = step(67);
    invalid.ratchets = 0;
    assert!(editor.set_step(1, Some(invalid)).is_err());

    assert_eq!(editor.pattern(), &before);
    assert_eq!(editor.revision(), 0);
}

#[test]
fn editor_supports_bounded_undo_and_redo() {
    let mut editor = PatternEditor::new(base_pattern(), 2).unwrap();

    editor.set_swing(0.1).unwrap();
    editor.set_length(5).unwrap();
    editor.rotate_right(1).unwrap();

    assert_eq!(editor.undo_depth(), 2);
    assert!(editor.undo());
    assert_eq!(editor.pattern().steps.len(), 5);
    assert!(editor.undo());
    assert_eq!(editor.pattern().steps.len(), 4);
    assert!(
        !editor.undo(),
        "oldest history entry should have been discarded"
    );

    assert!(editor.redo());
    assert_eq!(editor.pattern().steps.len(), 5);
}

#[test]
fn copy_paste_rotate_and_resize_are_deterministic() {
    let mut editor = PatternEditor::new(base_pattern(), 16).unwrap();

    editor.copy_step(0).unwrap();
    editor.paste_step(1).unwrap();
    assert_eq!(editor.pattern().steps[1], Some(step(60)));

    editor.rotate_left(1).unwrap();
    assert_eq!(editor.pattern().steps[0], Some(step(60)));
    assert_eq!(editor.pattern().steps[3], Some(step(60)));

    editor.set_length(6).unwrap();
    assert_eq!(editor.pattern().steps.len(), 6);
    assert_eq!(editor.pattern().steps[4], None);
    assert_eq!(editor.pattern().steps[5], None);

    editor.set_length(2).unwrap();
    assert_eq!(editor.pattern().steps.len(), 2);
}
