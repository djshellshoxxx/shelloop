use shelloop::{next_boundary_frame, QuantizeBoundary, QuantizedChange};

#[test]
fn immediate_boundary_uses_current_frame() {
    assert_eq!(
        next_boundary_frame(12_345, 48_000, 120.0, 4, QuantizeBoundary::Immediate).unwrap(),
        12_345
    );
}

#[test]
fn step_beat_and_bar_boundaries_are_sample_frame_based() {
    // 120 BPM at 48 kHz => 24,000 frames/beat; four steps/beat => 6,000/step.
    assert_eq!(
        next_boundary_frame(6_001, 48_000, 120.0, 4, QuantizeBoundary::Step).unwrap(),
        12_000
    );
    assert_eq!(
        next_boundary_frame(24_001, 48_000, 120.0, 4, QuantizeBoundary::Beat).unwrap(),
        48_000
    );
    assert_eq!(
        next_boundary_frame(
            96_001,
            48_000,
            120.0,
            4,
            QuantizeBoundary::Bar { beats_per_bar: 4 },
        )
        .unwrap(),
        192_000
    );
}

#[test]
fn exact_boundary_is_idempotent_not_deferred_to_the_next_one() {
    assert_eq!(
        next_boundary_frame(6_000, 48_000, 120.0, 4, QuantizeBoundary::Step).unwrap(),
        6_000
    );
    assert_eq!(
        next_boundary_frame(24_000, 48_000, 120.0, 4, QuantizeBoundary::Beat).unwrap(),
        24_000
    );
}

#[test]
fn odd_meter_bar_uses_requested_beats_per_bar() {
    // 7/4-like seven-beat bar at 120 BPM = 168,000 frames.
    assert_eq!(
        next_boundary_frame(
            168_001,
            48_000,
            120.0,
            4,
            QuantizeBoundary::Bar { beats_per_bar: 7 },
        )
        .unwrap(),
        336_000
    );
}

#[test]
fn pattern_boundary_uses_the_target_tracks_independent_length() {
    // Three 16th-note steps = 18,000 frames at these settings.
    assert_eq!(
        next_boundary_frame(
            18_001,
            48_000,
            120.0,
            4,
            QuantizeBoundary::Pattern { steps: 3 },
        )
        .unwrap(),
        36_000
    );
}

#[test]
fn quantized_change_carries_value_and_resolved_absolute_frame() {
    let change =
        QuantizedChange::new("scene-b", 25_000, 48_000, 120.0, 4, QuantizeBoundary::Beat).unwrap();

    assert_eq!(change.apply_at_frame, 48_000);
    assert_eq!(change.value, "scene-b");
}

#[test]
fn invalid_clock_and_boundary_values_are_rejected() {
    assert!(next_boundary_frame(0, 0, 120.0, 4, QuantizeBoundary::Beat).is_err());
    assert!(next_boundary_frame(0, 48_000, 0.0, 4, QuantizeBoundary::Beat).is_err());
    assert!(next_boundary_frame(0, 48_000, f64::NAN, 4, QuantizeBoundary::Beat).is_err());
    assert!(next_boundary_frame(0, 48_000, 120.0, 0, QuantizeBoundary::Step).is_err());
    assert!(next_boundary_frame(
        0,
        48_000,
        120.0,
        4,
        QuantizeBoundary::Bar { beats_per_bar: 0 }
    )
    .is_err());
    assert!(
        next_boundary_frame(0, 48_000, 120.0, 4, QuantizeBoundary::Pattern { steps: 0 }).is_err()
    );
}
