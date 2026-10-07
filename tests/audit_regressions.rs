//! Regression tests for the v1 audit findings.

use crossbeam_channel::{bounded, Receiver};
use shelloop::{
    apply_audio_message, capture_channel, AudioMessage, CompiledPattern, CompiledPatternRevision,
    Garbage, LibraryPattern, LockTarget, MidiEvent, MultiTrackEngine, MultiTrackProject,
    ParameterLock, Pattern, PatternEditor, PatternId, PatternScheduler, PatternStep,
    QuantizeBoundary, QuantizedChange, ResampleSource, Scene, SceneId, SceneTrackState,
    SessionController, SynthParamId, TrackDefinition, TrackId, TrackKind,
};

const RATE: u32 = 48_000;
const BAR: u64 = 96_000;

fn step(note: u8) -> Option<PatternStep> {
    Some(PatternStep {
        note,
        velocity: 0.8,
        gate: 0.5,
        probability: 1.0,
        ratchets: 1,
        microtiming_frames: 0,
    })
}

fn pattern(notes: &[Option<u8>]) -> Pattern {
    Pattern::new(
        "p",
        1,
        0.0,
        0,
        notes.iter().map(|note| note.and_then(step)).collect(),
    )
    .unwrap()
}

fn project() -> MultiTrackProject {
    let mut track = TrackDefinition::new(
        TrackId(1),
        "bass",
        TrackKind::Synth,
        pattern(&[Some(48), None, Some(50), None]),
    );
    let mut patch = shelloop::SynthPatch::legacy(shelloop::Oscillator::Saw);
    patch.amp_env.release_secs = 2.0;
    track.synth_patch = Some(patch);
    track.pattern_library = vec![LibraryPattern {
        id: PatternId(1),
        pattern: pattern(&[Some(60), Some(61), Some(62), Some(63)]),
    }];
    let mut project = MultiTrackProject::new(3, 120.0, 4, vec![track]);
    project.scenes = vec![Scene {
        id: SceneId(1),
        name: "B".into(),
        track_states: vec![SceneTrackState {
            track_id: TrackId(1),
            pattern_id: PatternId(1),
            muted: Some(true),
            gain: Some(0.5),
        }],
    }];
    project
}

struct Rig {
    controller: SessionController,
    receiver: Receiver<AudioMessage>,
    engine: MultiTrackEngine,
    garbage: crossbeam_channel::Sender<Garbage>,
}

impl Rig {
    fn new() -> Self {
        let project = project();
        let (sender, receiver) = bounded(16);
        let engine = MultiTrackEngine::from_project(RATE, 4, &project, None).unwrap();
        let controller = SessionController::new(project, None, RATE, sender).unwrap();
        let (garbage, _) = bounded(4);
        Self {
            controller,
            receiver,
            engine,
            garbage,
        }
    }

    fn run(&mut self, command: &str) -> Result<String, String> {
        let frame = self.engine.position_frame();
        let result = self.controller.execute(command, frame);
        self.drain();
        result
    }

    fn drain(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            apply_audio_message(&mut self.engine, message, &self.garbage);
        }
    }

    fn render(&mut self, frames: u64) {
        for _ in 0..frames {
            self.engine.next_stereo_frame();
        }
    }
}

fn revision(
    pattern: &Pattern,
    revision: u64,
    frame: u64,
) -> QuantizedChange<CompiledPatternRevision> {
    QuantizedChange {
        apply_at_frame: frame,
        value: CompiledPatternRevision {
            revision,
            pattern: CompiledPattern::from_pattern(pattern).unwrap(),
        },
    }
}

#[test]
fn variation_proposal_never_applies_to_another_pattern() {
    let mut rig = Rig::new();
    rig.run("variation preview --seed 5 --amount 0.5").unwrap();
    rig.run("pattern 1").unwrap();
    assert!(rig.run("variation accept").is_err());
    let library = &rig.controller.project().tracks[0].pattern_library[0].pattern;
    assert_eq!(library.steps[0].unwrap().note, 60, "pattern 1 untouched");
}

#[test]
fn rotate_moves_locks_and_shortening_drops_them() {
    let mut editor = PatternEditor::new(pattern(&[Some(48), None, Some(50), None]), 8).unwrap();
    let mut locked = editor.pattern().clone();
    let lock = ParameterLock {
        target: LockTarget::Synth(SynthParamId::FilterCutoff),
        value: 500.0,
    };
    locked.set_step_locks(0, vec![lock]);
    locked.set_step_locks(3, vec![lock]);
    editor.replace_pattern(locked).unwrap();
    editor.rotate_right(1).unwrap();
    assert_eq!(editor.pattern().step_locks(1), &[lock]);
    assert_eq!(
        editor.pattern().step_locks(0),
        &[lock],
        "step 3 wrapped to 0"
    );
    editor.rotate_left(1).unwrap();
    assert_eq!(editor.pattern().step_locks(0), &[lock]);
    assert_eq!(editor.pattern().step_locks(3), &[lock]);
    editor.set_length(2).unwrap();
    assert_eq!(editor.pattern().locks.len(), 1);
}

#[test]
fn short_step_field_names_are_accepted() {
    let mut rig = Rig::new();
    rig.run("step 1 vel 0.5").unwrap();
    rig.run("step 1 prob 0.5").unwrap();
    rig.run("step 1 ratchet 2").unwrap();
    let step = rig.controller.project().tracks[0].pattern.steps[0].unwrap();
    assert_eq!(
        (step.velocity, step.probability, step.ratchets),
        (0.5, 0.5, 2)
    );
}

#[test]
fn controller_follows_scene_changes() {
    let mut rig = Rig::new();
    rig.run("scene launch b now").unwrap();
    rig.render(1);
    assert_eq!(rig.engine.track_is_muted(TrackId(1)), Some(true));
    rig.controller
        .observe_active_scene(rig.engine.active_scene());
    assert!(rig.controller.project().tracks[0].muted);
    assert_eq!(rig.controller.editors().selected_pattern(), PatternId(1));
    // Toggle now unmutes, matching what is audible.
    rig.run("mute").unwrap();
    assert_eq!(rig.engine.track_is_muted(TrackId(1)), Some(false));
}

#[test]
fn pickup_mapping_keeps_tracking_its_own_motion() {
    let mut rig = Rig::new();
    rig.run("learn track.pan").unwrap();
    let cc = |value| MidiEvent::ControlChange {
        channel: 0,
        controller: 10,
        value,
    };
    rig.controller.handle_midi("Port", cc(64), 0, 0);
    rig.run("learn confirm").unwrap();
    for value in (0..=127).rev().step_by(5) {
        rig.controller.handle_midi("Port", cc(value), 10, 0);
    }
    rig.drain();
    let pan = rig.controller.project().tracks[0].pan;
    assert!(pan < -0.95, "pan followed the sweep to {pan}");
}

#[test]
fn save_as_keeps_sample_paths_valid() {
    let source = tempfile::tempdir().unwrap();
    let target = tempfile::tempdir().unwrap();
    let samples = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("projects");
    let original = samples.join("example-samples.json");
    let project = MultiTrackProject::load(&original).unwrap();
    let copy = source.path().join("song.json");
    std::fs::create_dir_all(source.path().join("samples")).unwrap();
    for name in ["kick.wav", "snare.wav", "hat.wav"] {
        std::fs::copy(
            samples.join("samples").join(name),
            source.path().join("samples").join(name),
        )
        .unwrap();
    }
    project.save_atomic(&copy).unwrap();
    let (sender, _receiver) = bounded(4);
    let mut controller = SessionController::new(project, Some(copy.clone()), RATE, sender).unwrap();
    let moved = target.path().join("moved.json");
    controller.save(Some(moved.clone())).unwrap();
    let reloaded = MultiTrackProject::load(&moved).unwrap();
    reloaded
        .load_sample_assets(target.path(), shelloop::DEFAULT_SAMPLE_MEMORY_BUDGET)
        .expect("sample paths resolve from the new folder");
}

#[test]
fn queued_revision_survives_a_scene_switch() {
    let project = project();
    let mut engine = MultiTrackEngine::from_project(RATE, 4, &project, None).unwrap();
    let edited = pattern(&[None, Some(55), None, None]);
    engine
        .queue_library_revision(TrackId(1), PatternId(0), revision(&edited, 5, 2 * BAR))
        .unwrap();
    engine.launch_scene(SceneId(1), BAR).unwrap();
    for _ in 0..BAR + 1 {
        engine.next_stereo_frame();
    }
    assert_eq!(engine.track_active_pattern(TrackId(1)), Some(PatternId(1)));
    engine
        .queue_library_revision(
            TrackId(1),
            PatternId(1),
            revision(&pattern(&[Some(70); 4]), 1, 0),
        )
        .unwrap();
    // Switch back to pattern 0 by restarting a scene-less engine path.
    engine
        .set_scenes(
            &[Scene {
                id: SceneId(2),
                name: "A".into(),
                track_states: vec![SceneTrackState {
                    track_id: TrackId(1),
                    pattern_id: PatternId(0),
                    muted: None,
                    gain: None,
                }],
            }],
            &[],
        )
        .unwrap();
    engine.launch_scene(SceneId(2), 0).unwrap();
    engine.next_stereo_frame();
    assert_eq!(engine.track_active_revision(TrackId(1)), Some(5));
}

#[test]
fn restart_applies_queued_revisions_immediately() {
    let project = project();
    let mut engine = MultiTrackEngine::from_project(RATE, 4, &project, None).unwrap();
    for _ in 0..90_000 {
        engine.next_stereo_frame();
    }
    engine
        .queue_pattern_revision(TrackId(1), revision(&pattern(&[None; 4]), 1, 96_000))
        .unwrap();
    engine.restart_all();
    assert_eq!(engine.track_active_revision(TrackId(1)), Some(1));
    assert_eq!(engine.track_queued_revision(TrackId(1)), None);
}

#[test]
fn notes_above_127_are_rejected() {
    let mut steps = vec![step(48)];
    steps[0].as_mut().unwrap().note = 200;
    assert!(Pattern::new("bad", 1, 0.0, 0, steps).is_err());
}

#[test]
fn late_scene_launch_requantizes_to_the_engine_playhead() {
    let project = project();
    let mut engine = MultiTrackEngine::from_project(RATE, 4, &project, None).unwrap();
    for _ in 0..(BAR + 10) {
        engine.next_stereo_frame();
    }
    let frame = engine
        .launch_scene_at(SceneId(1), QuantizeBoundary::Bar { beats_per_bar: 4 })
        .unwrap();
    assert_eq!(frame, 2 * BAR);
    assert_eq!(engine.queued_scene(), Some((SceneId(1), 2 * BAR)));
}

#[test]
fn pattern_switch_releases_instead_of_cutting_notes() {
    let project = project();
    let mut engine = MultiTrackEngine::from_project(RATE, 4, &project, None).unwrap();
    for _ in 0..100 {
        engine.next_stereo_frame();
    }
    let before = engine.next_stereo_frame().0.abs();
    assert!(before > 0.0);
    let silent = pattern(&[None; 4]);
    engine
        .queue_pattern_revision(TrackId(1), revision(&silent, 1, 0))
        .unwrap();
    // With a 2 s release the old note keeps sounding after the swap.
    let mut energy = 0.0;
    for _ in 0..1_000 {
        energy += engine.next_stereo_frame().0.abs();
    }
    assert!(energy > 1.0, "release tail continues, energy {energy}");
}

#[test]
fn microtiming_longer_than_a_loop_keeps_every_note() {
    let scheduler = PatternScheduler::new(RATE, 120.0, 4, 1).unwrap();
    let mut steps = vec![step(60)];
    steps[0].as_mut().unwrap().microtiming_frames = 13_000;
    let pattern = Pattern::new("micro", 1, 0.0, 0, steps).unwrap();
    let mut count = 0;
    let mut start = 0;
    while start < 96_000 {
        count += scheduler.schedule_block(&pattern, start, 2048).len();
        start += 2048;
    }
    // One note per 6000-frame loop, shifted by 13000 frames.
    assert_eq!(count, (96_000 - 13_000) / 6_000 + 1);
}

#[test]
fn capture_ends_when_the_transport_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let (control, mut tap, returns) = capture_channel();
    let mut resampler = shelloop::Resampler::new(dir.path().to_path_buf(), RATE, control, returns);
    let clock = shelloop::CaptureClock {
        sample_rate: RATE,
        bpm: 120.0,
        steps_per_beat: 4,
        beats_per_bar: 4,
    };
    let request = shelloop::ResampleRequest {
        source: ResampleSource::Master,
        start_boundary: QuantizeBoundary::Immediate,
        stop: shelloop::ResampleStop::Bars(1),
        normalize: false,
        destination: shelloop::ResampleDestination::AssetOnly,
    };
    resampler.arm(request, 0, clock).unwrap();
    tap.poll_control();
    for frame in 0..1_000 {
        tap.process_frame(frame, (0.1, 0.1), None);
    }
    // Restart: frames begin again at 0.
    for frame in 0..1_000 {
        tap.process_frame(frame, (0.2, 0.2), None);
    }
    assert!(!tap.is_capturing());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let finished = loop {
        if let Some(done) = resampler.poll(0) {
            break done;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(5));
    };
    assert_eq!(finished.summary.frames_written, 1_000);
}
