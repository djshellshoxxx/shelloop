use shelloop::{Pattern, PatternScheduler, PatternStep};

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

#[test]
fn pattern_validation_rejects_unbounded_values() {
    assert!(Pattern::new("empty", 0, 0.0, 1, vec![]).is_err());
    assert!(Pattern::new("too-long", 0, 0.0, 1, vec![None; 257]).is_err());

    let mut bad = step(60);
    bad.probability = 1.1;
    assert!(Pattern::new("bad-probability", 0, 0.0, 1, vec![Some(bad)]).is_err());

    let mut bad = step(60);
    bad.ratchets = 9;
    assert!(Pattern::new("bad-ratchets", 0, 0.0, 1, vec![Some(bad)]).is_err());

    assert!(Pattern::new("bad-swing", 0, 0.76, 1, vec![Some(step(60))]).is_err());
}

#[test]
fn independent_pattern_length_repeats_without_block_boundary_duplicates() {
    let pattern = Pattern::new(
        "three",
        11,
        0.0,
        1,
        vec![Some(step(60)), None, Some(step(64))],
    )
    .unwrap();
    let scheduler = PatternScheduler::new(48_000, 120.0, 4, 99).unwrap();

    // 6,000 frames/step, 18,000-frame pattern. Step 0 occurs at 0 and 18,000.
    let first = scheduler.schedule_block(&pattern, 17_900, 100);
    let second = scheduler.schedule_block(&pattern, 18_000, 100);

    assert!(first.is_empty());
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].frame_offset, 0);
    assert_eq!(second[0].note, 60);
}

#[test]
fn swing_delays_odd_steps_and_microtiming_is_signed() {
    let mut early = step(62);
    early.microtiming_frames = -50;
    let pattern = Pattern::new("swing", 3, 0.25, 1, vec![None, Some(early)]).unwrap();
    let scheduler = PatternScheduler::new(48_000, 120.0, 4, 1).unwrap();

    // Step 1 base=6000, swing delay=1500, microtiming=-50 => 7450.
    let events = scheduler.schedule_block(&pattern, 7_400, 100);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].frame_offset, 50);
}

#[test]
fn negative_microtiming_before_transport_zero_is_dropped_not_wrapped() {
    let mut early = step(60);
    early.microtiming_frames = -100;
    let pattern = Pattern::new("early", 7, 0.0, 1, vec![Some(early)]).unwrap();
    let scheduler = PatternScheduler::new(48_000, 120.0, 4, 1).unwrap();

    let events = scheduler.schedule_block(&pattern, 0, 512);
    assert!(events.is_empty());
}

#[test]
fn ratchets_are_evenly_distributed_and_bounded() {
    let mut ratchet = step(36);
    ratchet.ratchets = 4;
    ratchet.gate = 0.5;
    let pattern = Pattern::new("ratchet", 5, 0.0, 1, vec![Some(ratchet)]).unwrap();
    let scheduler = PatternScheduler::new(48_000, 120.0, 4, 1).unwrap();

    let events = scheduler.schedule_block(&pattern, 0, 6_000);
    let offsets: Vec<u32> = events.iter().map(|event| event.frame_offset).collect();
    assert_eq!(offsets, vec![0, 1_500, 3_000, 4_500]);
    assert!(events.iter().all(|event| event.duration_frames == 750));
}

#[test]
fn probability_is_deterministic_across_different_block_partitions() {
    let mut probabilistic = step(42);
    probabilistic.probability = 0.5;
    let pattern = Pattern::new("prob", 1234, 0.0, 1, vec![Some(probabilistic)]).unwrap();
    let scheduler = PatternScheduler::new(48_000, 120.0, 4, 0xDEADBEEF).unwrap();

    let whole = scheduler.schedule_block(&pattern, 0, 60_000);
    let mut partitioned = Vec::new();
    for start in (0..60_000u64).step_by(3_000) {
        for mut event in scheduler.schedule_block(&pattern, start, 3_000) {
            event.absolute_frame = start + event.frame_offset as u64;
            partitioned.push(event);
        }
    }

    let whole_frames: Vec<u64> = whole.iter().map(|event| event.absolute_frame).collect();
    let partitioned_frames: Vec<u64> = partitioned
        .iter()
        .map(|event| event.absolute_frame)
        .collect();
    assert_eq!(whole_frames, partitioned_frames);
}

#[test]
fn probability_extremes_are_exact() {
    let mut never = step(60);
    never.probability = 0.0;
    let mut always = step(61);
    always.probability = 1.0;
    let pattern = Pattern::new("extremes", 8, 0.0, 1, vec![Some(never), Some(always)]).unwrap();
    let scheduler = PatternScheduler::new(48_000, 120.0, 4, 1).unwrap();

    let events = scheduler.schedule_block(&pattern, 0, 12_000);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].note, 61);
    assert_eq!(events[0].absolute_frame, 6_000);
}
