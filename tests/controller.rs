use crossbeam_channel::{bounded, Receiver};
use shelloop::{
    apply_audio_message, AudioMessage, EffectConfig, EffectKind, EngineTelemetry, Garbage,
    LibraryPattern, MidiEvent, MultiTrackEngine, MultiTrackProject, Pattern, PatternId,
    PatternStep, Scene, SceneId, SceneTrackState, SessionController, TrackDefinition, TrackId,
    TrackKind,
};

const RATE: u32 = 48_000;

fn step() -> Option<PatternStep> {
    Some(PatternStep {
        note: 48,
        velocity: 0.8,
        gate: 0.5,
        probability: 1.0,
        ratchets: 1,
        microtiming_frames: 0,
    })
}

fn project() -> MultiTrackProject {
    let pattern = Pattern::new("p", 1, 0.0, 0, vec![step(), None, None, None]).unwrap();
    let mut bass = TrackDefinition::new(TrackId(1), "bass", TrackKind::Synth, pattern.clone());
    bass.inserts = vec![EffectConfig::new(EffectKind::Delay)];
    bass.pattern_library = vec![LibraryPattern {
        id: PatternId(1),
        pattern: pattern.clone(),
    }];
    let lead = TrackDefinition::new(TrackId(2), "lead", TrackKind::Synth, pattern);
    let mut project = MultiTrackProject::new(5, 120.0, 4, vec![bass, lead]);
    project.scenes = vec![Scene {
        id: SceneId(1),
        name: "Drop".into(),
        track_states: vec![SceneTrackState {
            track_id: TrackId(1),
            pattern_id: PatternId(1),
            muted: None,
            gain: None,
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
        let (sender, receiver) = bounded(8);
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
        while let Ok(message) = self.receiver.try_recv() {
            apply_audio_message(&mut self.engine, message, &self.garbage);
        }
        result
    }

    fn render(&mut self, frames: u64) {
        for _ in 0..frames {
            self.engine.next_stereo_frame();
        }
    }
}

#[test]
fn mixer_commands_reach_engine_and_project_mirror() {
    let mut rig = Rig::new();
    rig.run("gain 0.5").unwrap();
    rig.run("track 2").unwrap();
    rig.run("mute").unwrap();
    rig.run("master 0.8").unwrap();
    assert_eq!(rig.engine.track_base_gain(TrackId(1)), Some(0.5));
    assert_eq!(rig.engine.track_is_muted(TrackId(2)), Some(true));
    assert_eq!(rig.engine.master_gain(), 0.8);
    assert_eq!(rig.controller.project().tracks[0].gain, 0.5);
    assert!(rig.controller.project().tracks[1].muted);
    assert!(rig.run("gain 7").is_err());
    assert!(rig.run("bogus").is_err());
}

#[test]
fn pattern_edits_and_locks_are_queued_as_revisions() {
    let mut rig = Rig::new();
    rig.run("step 2 toggle").unwrap();
    rig.run("lock filter.cutoff 4800").unwrap();
    rig.run("lock track.gain 0.5").unwrap();
    let locks = rig.run("locks").unwrap();
    assert!(locks.contains("filter_cutoff=4800"), "{locks}");
    rig.render(20_000);
    assert!(rig.engine.track_active_revision(TrackId(1)).unwrap() >= 3);
    let pattern = &rig.controller.project().tracks[0].pattern;
    assert_eq!(pattern.step_locks(1).len(), 2);
    rig.run("unlock track.gain").unwrap();
    assert!(rig.run("unlock track.gain").is_err());
    assert!(rig.run("lock oscillator 1").is_err());
    assert!(rig.run("lock fx.0.time_ms 120").is_ok());
}

#[test]
fn effect_and_scene_commands() {
    let mut rig = Rig::new();
    rig.run("fx track 0 mix 0.5").unwrap();
    assert_eq!(
        rig.engine.effect_param(
            shelloop::EffectLocation::Track(TrackId(1)),
            shelloop::EffectSlotId(0),
            shelloop::EffectParamId(3)
        ),
        Some(0.5)
    );
    assert!(rig.run("fx track 0 nonsense 1").is_err());
    assert!(rig.run("fx list").unwrap().contains("delay"));
    rig.run("scene launch drop now").unwrap();
    rig.render(1);
    assert_eq!(rig.engine.active_scene(), Some(SceneId(1)));
    assert_eq!(
        rig.engine.track_active_pattern(TrackId(1)),
        Some(PatternId(1))
    );
    assert!(rig.run("scene launch nowhere").is_err());
}

#[test]
fn midi_learn_maps_cc_to_parameter_and_persists() {
    let mut rig = Rig::new();
    rig.run("learn filter.cutoff").unwrap();
    let cc = |value| MidiEvent::ControlChange {
        channel: 0,
        controller: 74,
        value,
    };
    let disposition = rig.controller.handle_midi("Port", cc(10), 0, 0);
    assert!(disposition.consumed);
    rig.run("learn confirm").unwrap();
    assert_eq!(rig.controller.project().midi_mappings.len(), 1);
    // Pickup: the cutoff starts at its legacy value (20 Hz), so a hardware
    // value at 0 crosses it and takes over.
    rig.controller.handle_midi("Port", cc(0), 10, 0);
    rig.controller.handle_midi("Port", cc(127), 20, 0);
    while let Ok(message) = rig.receiver.try_recv() {
        apply_audio_message(&mut rig.engine, message, &rig.garbage);
    }
    let patch = rig.engine.track_base_synth_patch(TrackId(1)).unwrap();
    assert!(
        patch.filter.cutoff_hz > 10_000.0,
        "{}",
        patch.filter.cutoff_hz
    );
    assert!(rig.run("mappings").unwrap().contains("1"));
}

#[test]
fn full_queue_is_reported_without_blocking() {
    let project = project();
    let (sender, _receiver) = bounded(1);
    let mut controller = SessionController::new(project, None, RATE, sender).unwrap();
    controller.execute("gain 0.5", 0).unwrap();
    assert!(controller.execute("gain 0.6", 0).is_err());
    assert_eq!(controller.dropped_messages(), 1);
}

#[test]
fn variation_preview_and_accept_round_trip() {
    let mut rig = Rig::new();
    rig.run("variation lock rhythm on").unwrap();
    let preview = rig.run("variation preview --seed 3 --amount 1").unwrap();
    assert!(preview.contains("text preview"), "{preview}");
    rig.run("variation accept").unwrap();
    rig.run("undo").unwrap();
}

#[test]
fn telemetry_snapshot_reflects_engine() {
    let mut rig = Rig::new();
    let telemetry = EngineTelemetry::default();
    rig.render(1000);
    telemetry.publish(&rig.engine);
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.position_frame, 1000);
    assert_eq!(snapshot.tracks.len(), 2);
    assert_eq!(snapshot.tracks[1].id, TrackId(2));
    assert!(snapshot.playing);
}

#[test]
fn save_writes_a_loadable_project() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("song.json");
    let mut rig = Rig::new();
    rig.run("step 3 toggle").unwrap();
    rig.run(&format!("save {}", path.display())).unwrap();
    let loaded = MultiTrackProject::load(&path).unwrap();
    assert!(loaded.tracks[0].pattern.steps[2].is_some());
}

#[test]
fn named_presets_capture_live_edits_and_load_only_after_queue_acceptance() {
    let directory = tempfile::tempdir().unwrap();
    let project_path = directory.path().join("song.json");
    let mut rig = Rig::new();
    rig.run(&format!("save {}", project_path.display()))
        .unwrap();
    rig.run("synth output_gain 0.42").unwrap();
    rig.run("mod add lfo1 filter_cutoff 0.5 25").unwrap();
    rig.run("preset save WarmPad").unwrap();
    assert!(rig.run("preset list").unwrap().contains("WarmPad"));
    assert!(rig.run("preset save WarmPad").is_err());
    rig.run("synth output_gain 0.8").unwrap();
    rig.run("mod clear").unwrap();
    rig.run("preset load WarmPad").unwrap();
    assert_eq!(
        rig.controller.project().tracks[0]
            .synth_patch
            .unwrap()
            .output_gain,
        0.42
    );
    assert_eq!(
        rig.engine
            .track_base_synth_patch(TrackId(1))
            .unwrap()
            .output_gain,
        0.42
    );
    rig.run("save").unwrap();
    assert_eq!(
        MultiTrackProject::load(project_path).unwrap().tracks[0]
            .synth_patch
            .unwrap()
            .output_gain,
        0.42
    );
    assert_eq!(
        rig.controller.project().tracks[0]
            .synth_patch
            .unwrap()
            .modulation
            .routes()
            .len(),
        1
    );
    let (sender, _receiver) = bounded(0);
    let mut blocked = SessionController::new(
        project(),
        Some(directory.path().join("blocked.json")),
        RATE,
        sender,
    )
    .unwrap();
    assert!(blocked.execute("preset load WarmPad", 0).is_err());
    assert_eq!(blocked.project().tracks[0].synth_patch, None);
    rig.run("preset delete WarmPad").unwrap();
    assert!(rig.run("preset load WarmPad").is_err());
    assert!(rig.run("preset save ../outside").is_err());
}

#[test]
fn all_eight_modulation_routes_are_editable_and_visible_in_the_tui() {
    let mut rig = Rig::new();
    for _ in 0..8 {
        rig.run("mod add macro1 filter_cutoff -0.1").unwrap();
    }
    assert!(rig.run("mod add macro1 filter_cutoff -0.1").is_err());
    let mut ui = shelloop::workstation::UiState::new(true);
    let snapshot = shelloop::workstation::build_snapshot(
        &rig.controller,
        &Default::default(),
        &mut ui,
        &Default::default(),
    );
    let routes: Vec<_> = snapshot
        .inspector
        .rows
        .iter()
        .filter(|(name, _)| name.starts_with("mod "))
        .collect();
    assert_eq!(routes.len(), 8);
    assert!(routes
        .iter()
        .all(|(_, value)| value.contains("FilterCutoff") && value.contains("Hz")));
    rig.run("mod set 8 velocity amplitude 0.2").unwrap();
    rig.run("mod off 8").unwrap();
    rig.run("mod remove 1").unwrap();
    assert_eq!(
        rig.controller.project().tracks[0]
            .synth_patch
            .unwrap()
            .modulation
            .routes()
            .len(),
        7
    );
}
