//! Spec 15 performance macros: assignment, validation, live moves and
//! scene recall.

use crossbeam_channel::bounded;
use shelloop::{
    apply_audio_message, EffectConfig, EffectKind, EffectLocation, EffectMacro, EffectSlotId,
    MultiTrackEngine, MultiTrackProject, Pattern, PatternId, PatternStep, Scene, SceneId,
    SceneMacroValue, SceneTrackState, SessionController, TrackDefinition, TrackId, TrackKind,
};

const RATE: u32 = 48_000;
const BAR: u64 = RATE as u64 * 2; // 4 beats at 120 bpm

fn project() -> MultiTrackProject {
    let step = Some(PatternStep {
        note: 48,
        velocity: 0.8,
        gate: 0.5,
        probability: 1.0,
        ratchets: 1,
        microtiming_frames: 0,
    });
    let pattern = Pattern::new("p", 1, 0.0, 0, vec![step, None, None, None]).unwrap();
    let mut track = TrackDefinition::new(TrackId(1), "lead", TrackKind::Synth, pattern);
    track.inserts = vec![
        EffectConfig::new(EffectKind::Delay),
        EffectConfig::new(EffectKind::Filter),
    ];
    let mut project = MultiTrackProject::new(3, 120.0, 4, vec![track]);
    project.effects.master = vec![EffectConfig::new(EffectKind::Chorus)];
    project.scenes = vec![
        Scene {
            id: SceneId(1),
            name: "Open".into(),
            track_states: vec![SceneTrackState {
                track_id: TrackId(1),
                pattern_id: PatternId(0),
                muted: None,
                gain: None,
            }],
        },
        Scene {
            id: SceneId(2),
            name: "Dark".into(),
            track_states: Vec::new(),
        },
    ];
    project.effect_macros = vec![
        EffectMacro {
            location: EffectLocation::Track(TrackId(1)),
            index: 1,
            slot: 1,
            param: "cutoff".into(),
            value: 1.0,
            scene_values: vec![
                SceneMacroValue {
                    scene: SceneId(1),
                    value: 1.0,
                },
                SceneMacroValue {
                    scene: SceneId(2),
                    value: 0.0,
                },
            ],
        },
        EffectMacro {
            location: EffectLocation::Master,
            index: 2,
            slot: 0,
            param: "mix".into(),
            value: 0.5,
            scene_values: vec![],
        },
    ];
    project
}

#[test]
fn valid_macros_round_trip_and_invalid_ones_are_rejected() {
    let good = project();
    good.validate().unwrap();
    let json = serde_json::to_string(&good).unwrap();
    assert!(json.contains("effect_macros") && json.contains("scene_values"));
    assert_eq!(
        serde_json::from_str::<MultiTrackProject>(&json).unwrap(),
        good
    );
    let legacy = MultiTrackProject::new(1, 120.0, 4, good.tracks.clone());
    assert!(!serde_json::to_string(&legacy)
        .unwrap()
        .contains("effect_macros"));

    let bad = |mutate: &dyn Fn(&mut MultiTrackProject)| {
        let mut p = project();
        mutate(&mut p);
        p.validate().unwrap_err()
    };
    assert!(bad(&|p| p.effect_macros[0].index = 3).contains("index"));
    assert!(bad(&|p| p.effect_macros[0].index = 0).contains("index"));
    assert!(bad(&|p| p.effect_macros[0].slot = 9).contains("does not exist"));
    assert!(bad(&|p| p.effect_macros[0].param = "wobble".into()).contains("unknown"));
    assert!(bad(&|p| p.effect_macros[0].value = 1.5).contains("between"));
    assert!(
        bad(&|p| p.effect_macros[0].scene_values[0].scene = SceneId(9)).contains("unknown scene")
    );
    assert!(bad(&|p| p.effect_macros[0].scene_values[1].scene = SceneId(1)).contains("two values"));
    assert!(bad(&|p| {
        let mut clone = p.effect_macros[0].clone();
        clone.slot = 0;
        clone.param = "feedback".into();
        p.effect_macros.push(clone);
    })
    .contains("two macros"));
    assert!(
        bad(&|p| p.effect_macros[0].location = EffectLocation::Send(1)).contains("does not exist")
    );
}

#[test]
fn scene_launch_recalls_macro_positions_through_the_parameter_path() {
    let p = project();
    let mut engine = MultiTrackEngine::from_project(RATE, 4, &p, None).unwrap();
    let location = EffectLocation::Track(TrackId(1));
    let cutoff = shelloop::find_effect_param(EffectKind::Filter, "cutoff").unwrap();
    let initial = engine
        .effect_param(location, EffectSlotId(1), cutoff)
        .unwrap();
    assert_eq!(
        initial, 2_000.0,
        "project load does not itself apply macros"
    );

    engine.launch_scene(SceneId(2), BAR).unwrap();
    for _ in 0..BAR - 1 {
        engine.next_stereo_frame();
    }
    assert_eq!(
        engine.effect_param(location, EffectSlotId(1), cutoff),
        Some(2_000.0)
    );
    for _ in 0..2_000 {
        engine.next_stereo_frame();
    }
    let dark = engine
        .effect_param(location, EffectSlotId(1), cutoff)
        .unwrap();
    assert!(
        (dark - 20.0).abs() < 0.5,
        "macro 0.0 maps to the minimum cutoff, got {dark}"
    );

    engine.launch_scene(SceneId(1), BAR * 2).unwrap();
    for _ in 0..BAR + 2_000 {
        engine.next_stereo_frame();
    }
    let open = engine
        .effect_param(location, EffectSlotId(1), cutoff)
        .unwrap();
    assert!(
        (open - 20_000.0).abs() < 5.0,
        "macro 1.0 maps to the maximum, got {open}"
    );
    // Macro 2 has no scene value, so scenes leave the master mix alone.
    let mix = shelloop::find_effect_param(EffectKind::Chorus, "mix").unwrap();
    assert_eq!(
        engine.effect_param(EffectLocation::Master, EffectSlotId(0), mix),
        Some(0.5)
    );
}

#[test]
fn controller_assigns_moves_and_removes_macros() {
    let mut p = project();
    p.effect_macros.clear();
    let (sender, receiver) = bounded(8);
    let mut engine = MultiTrackEngine::from_project(RATE, 4, &p, None).unwrap();
    let mut controller = SessionController::new(p, None, RATE, sender).unwrap();
    let (garbage, _) = bounded(4);
    let mut run = |line: &str| {
        let result = controller.execute(line, engine.position_frame());
        while let Ok(message) = receiver.try_recv() {
            apply_audio_message(&mut engine, message, &garbage);
        }
        (
            result,
            engine.effect_param(
                EffectLocation::Track(TrackId(1)),
                EffectSlotId(1),
                shelloop::find_effect_param(EffectKind::Filter, "cutoff").unwrap(),
            ),
        )
    };
    assert_eq!(run("fxmacro list").0.unwrap(), "no effect macros");
    assert!(
        run("fxmacro track 1 0.5").0.is_err(),
        "unassigned macros cannot move"
    );
    run("fxmacro assign track 1 1 cutoff").0.unwrap();
    assert!(run("fxmacro assign track 1 1 nonsense").0.is_err());
    assert!(run("fxmacro assign track 3 1 cutoff").0.is_err());
    assert!(run("fxmacro assign track 1 7 cutoff").0.is_err());
    let (message, cutoff) = run("fxmacro track 1 1.0");
    assert!(message.unwrap().contains("macro 1 = 1.00"));
    assert_eq!(cutoff, Some(20_000.0));
    let (_, cutoff) = run("fxmacro track 1 0.0");
    assert_eq!(cutoff, Some(20.0));
    assert!(
        run("fxmacro track 1 2").0.is_err(),
        "out-of-range positions are rejected"
    );
    let (_, cutoff) = run("fxmacro track 1 0.5");
    // Log curve: geometric mean of 20 Hz and 20 kHz.
    assert!((cutoff.unwrap() - 632.5).abs() < 1.0, "{cutoff:?}");
    assert!(run("fxmacro list").0.unwrap().contains("cutoff = 0.50"));
    run("fxmacro remove track 1").0.unwrap();
    assert!(run("fxmacro remove track 1").0.is_err());
    // Chain macros are independent: master macro 2 works without macro 1.
    run("fxmacro assign master 2 0 depth").0.unwrap();
    assert!(run("fxmacro master 2 0.25")
        .0
        .unwrap()
        .contains("macro 2 = 0.25"));
}
