use shelloop::{Oscillator, Project, Scheduler, StepEvent, SynthVoice, Track};

#[test]
fn scheduler_places_step_zero_at_block_start() {
    let scheduler = Scheduler::new(48_000, 120.0, 4);
    let events = vec![StepEvent { step: 0, note: 60, velocity: 1.0 }];

    let scheduled = scheduler.schedule_block(0, 512, &events);

    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].frame_offset, 0);
    assert_eq!(scheduled[0].note, 60);
}

#[test]
fn scheduler_places_later_steps_at_exact_sample_frames() {
    let scheduler = Scheduler::new(48_000, 120.0, 4);
    let events = vec![StepEvent { step: 1, note: 62, velocity: 0.75 }];

    // At 120 BPM, one beat is 24,000 frames. Four steps per beat => 6,000 frames/step.
    let scheduled = scheduler.schedule_block(5_900, 256, &events);

    assert_eq!(scheduled.len(), 1);
    assert_eq!(scheduled[0].frame_offset, 100);
}

#[test]
fn synth_note_on_generates_finite_nonzero_audio() {
    let mut voice = SynthVoice::new(48_000.0, Oscillator::Sine);
    voice.note_on(440.0, 0.8);

    let audio = voice.render(256);

    assert_eq!(audio.len(), 256);
    assert!(audio.iter().all(|sample| sample.is_finite()));
    assert!(audio.iter().any(|sample| sample.abs() > 0.0001));
    assert!(audio.iter().all(|sample| sample.abs() <= 1.0));
}

#[test]
fn valid_project_passes_validation() {
    let project = Project {
        version: 1,
        bpm: 128.0,
        tracks: vec![Track { name: "bass".into() }],
    };

    assert!(project.validate().is_ok());
}

#[test]
fn invalid_tempo_is_rejected() {
    let project = Project { version: 1, bpm: 0.0, tracks: Vec::new() };

    assert!(project.validate().is_err());
}
