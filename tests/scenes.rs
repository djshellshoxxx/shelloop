use shelloop::{
    resolve_scene, ChainId, ChainStep, CompiledPattern, CompiledPatternRevision, LibraryPattern,
    MultiTrackEngine, MultiTrackProject, Pattern, PatternId, PatternStep, QuantizedChange, Scene,
    SceneChain, SceneId, SceneTrackState, TrackDefinition, TrackId, TrackKind,
};

const RATE: u32 = 48_000;
// 120 BPM, 4/4: 96_000 frames per bar.
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

fn pattern(name: &str, note: u8) -> Pattern {
    Pattern::new(name, 3, 0.0, 0, vec![step(note), None, None, None]).unwrap()
}

fn track(id: u16) -> TrackDefinition {
    let mut track = TrackDefinition::new(
        TrackId(id),
        format!("t{id}"),
        TrackKind::Synth,
        pattern("main", 48),
    );
    track.pattern_library = vec![
        LibraryPattern {
            id: PatternId(1),
            pattern: pattern("verse", 50),
        },
        LibraryPattern {
            id: PatternId(2),
            pattern: pattern("drop", 52),
        },
    ];
    track
}

fn scene(id: u16, name: &str, pattern_id: u16, muted: Option<bool>) -> Scene {
    Scene {
        id: SceneId(id),
        name: name.into(),
        track_states: vec![
            SceneTrackState {
                track_id: TrackId(1),
                pattern_id: PatternId(pattern_id),
                muted,
                gain: None,
            },
            SceneTrackState {
                track_id: TrackId(2),
                pattern_id: PatternId(pattern_id),
                muted: None,
                gain: Some(0.5),
            },
        ],
    }
}

fn project() -> MultiTrackProject {
    let mut project = MultiTrackProject::new(9, 120.0, 4, vec![track(1), track(2)]);
    project.scenes = vec![
        scene(1, "Intro", 0, None),
        scene(2, "Verse", 1, Some(true)),
        scene(3, "Drop", 2, Some(false)),
    ];
    project.chains = vec![SceneChain {
        id: ChainId(1),
        name: "main".into(),
        steps: vec![
            ChainStep {
                scene_id: SceneId(2),
                repeats: 2,
                follow: None,
            },
            ChainStep {
                scene_id: SceneId(3),
                repeats: 1,
                follow: None,
            },
        ],
        loop_chain: true,
    }];
    project
}

fn engine() -> MultiTrackEngine {
    MultiTrackEngine::from_project(RATE, 8, &project(), None).unwrap()
}

fn render(engine: &mut MultiTrackEngine, frames: u64) {
    for _ in 0..frames {
        engine.next_stereo_frame();
    }
}

#[test]
fn scene_switches_all_tracks_on_the_same_frame_and_not_early() {
    let mut engine = engine();
    render(&mut engine, 1_000);
    engine.launch_scene(SceneId(2), BAR).unwrap();
    assert_eq!(engine.queued_scene(), Some((SceneId(2), BAR)));
    render(&mut engine, BAR - 1_000);
    assert_eq!(engine.track_active_pattern(TrackId(1)), Some(PatternId(0)));
    assert_eq!(engine.track_active_pattern(TrackId(2)), Some(PatternId(0)));
    assert_eq!(engine.active_scene(), None);
    render(&mut engine, 1);
    assert_eq!(engine.track_active_pattern(TrackId(1)), Some(PatternId(1)));
    assert_eq!(engine.track_active_pattern(TrackId(2)), Some(PatternId(1)));
    assert_eq!(engine.track_is_muted(TrackId(1)), Some(true));
    assert_eq!(engine.track_base_gain(TrackId(2)), Some(0.5));
    assert_eq!(engine.active_scene(), Some(SceneId(2)));
    assert_eq!(engine.queued_scene(), None);
}

#[test]
fn exact_boundary_launch_applies_once() {
    let mut engine = engine();
    engine.launch_scene(SceneId(3), 0).unwrap();
    render(&mut engine, 1);
    assert_eq!(engine.active_scene(), Some(SceneId(3)));
    assert_eq!(engine.queued_scene(), None);
    render(&mut engine, BAR);
    assert_eq!(engine.track_active_pattern(TrackId(1)), Some(PatternId(2)));
}

#[test]
fn invalid_scene_references_are_rejected_before_activation() {
    let mut bad = project();
    bad.scenes[0].track_states[0].pattern_id = PatternId(9);
    assert!(bad.validate().is_err());
    assert!(MultiTrackEngine::from_project(RATE, 8, &bad, None).is_err());

    let mut bad = project();
    bad.scenes[0].track_states[0].track_id = TrackId(42);
    assert!(bad.validate().is_err());

    let mut engine = engine();
    assert!(engine.launch_scene(SceneId(77), 0).is_err());
    assert_eq!(engine.queued_scene(), None);
}

#[test]
fn chain_advances_exactly_on_bar_boundaries_and_loops() {
    let mut engine = engine();
    render(&mut engine, 10);
    let start = engine.start_chain(ChainId(1), 10).unwrap();
    assert_eq!(start, BAR);
    render(&mut engine, BAR - 10);
    assert_eq!(engine.active_scene(), None);
    render(&mut engine, 1);
    assert_eq!(engine.active_scene(), Some(SceneId(2)));
    // Verse lasts two bars.
    render(&mut engine, 2 * BAR - 1);
    assert_eq!(engine.active_scene(), Some(SceneId(2)));
    render(&mut engine, 1);
    assert_eq!(engine.active_scene(), Some(SceneId(3)));
    assert_eq!(engine.chain_status().unwrap().step, 1);
    // Drop lasts one bar, then the chain loops to the verse.
    render(&mut engine, BAR);
    assert_eq!(engine.active_scene(), Some(SceneId(2)));
    let status = engine.chain_status().unwrap();
    assert_eq!(status.step, 0);
    assert_eq!(status.next_change_frame, 6 * BAR);
}

#[test]
fn manual_launch_stops_running_chain() {
    let mut engine = engine();
    engine.start_chain(ChainId(1), 0).unwrap();
    render(&mut engine, 10);
    assert!(engine.chain_status().is_some());
    engine.launch_scene(SceneId(1), BAR).unwrap();
    assert!(engine.chain_status().is_none());
    render(&mut engine, 3 * BAR);
    assert_eq!(engine.active_scene(), Some(SceneId(1)));
}

#[test]
fn restart_discards_queued_scene_and_stops_chain() {
    let mut engine = engine();
    engine.start_chain(ChainId(1), 0).unwrap();
    engine.launch_scene(SceneId(3), BAR).unwrap();
    engine.restart_all();
    assert_eq!(engine.queued_scene(), None);
    assert!(engine.chain_status().is_none());
    assert_eq!(engine.position_frame(), 0);
}

#[test]
fn editing_inactive_pattern_does_not_change_active_audio() {
    let mut reference = engine();
    let mut edited = engine();
    let silent = Pattern::new("x", 1, 0.0, 0, vec![None; 4]).unwrap();
    edited
        .queue_library_revision(
            TrackId(1),
            PatternId(2),
            QuantizedChange {
                apply_at_frame: 0,
                value: CompiledPatternRevision {
                    revision: 1,
                    pattern: CompiledPattern::from_pattern(&silent).unwrap(),
                },
            },
        )
        .unwrap();
    for _ in 0..BAR {
        assert_eq!(reference.next_stereo_frame(), edited.next_stereo_frame());
    }
}

#[test]
fn scene_changes_are_identical_across_block_sizes() {
    let mut a = engine();
    let mut b = engine();
    a.start_chain(ChainId(1), 0).unwrap();
    b.start_chain(ChainId(1), 0).unwrap();
    let total = 4 * BAR;
    let left: Vec<_> = (0..total).map(|_| a.next_stereo_frame()).collect();
    let mut right = Vec::with_capacity(total as usize);
    let mut block = 1;
    while (right.len() as u64) < total {
        for _ in 0..block.min(total - right.len() as u64) {
            right.push(b.next_stereo_frame());
        }
        block = block * 7 % 4_093 + 1;
    }
    assert_eq!(left, right);
}

#[test]
fn project_supports_32_scenes_and_128_step_chains_and_roundtrips() {
    let mut project = project();
    project.scenes = (0..32)
        .map(|index| scene(index, &format!("s{index}"), index % 3, None))
        .collect();
    project.chains[0].steps = (0..128)
        .map(|index| ChainStep {
            scene_id: SceneId(index % 32),
            repeats: 1,
            follow: None,
        })
        .collect();
    project.validate().unwrap();
    let json = serde_json::to_string(&project).unwrap();
    let loaded: MultiTrackProject = serde_json::from_str(&json).unwrap();
    assert_eq!(loaded, project);
    assert_eq!(resolve_scene(&project.scenes, "S5").unwrap(), SceneId(5));
    assert_eq!(resolve_scene(&project.scenes, "7").unwrap(), SceneId(7));
    assert!(resolve_scene(&project.scenes, "nope").is_err());
}

#[test]
fn follow_actions_are_reserved() {
    let mut project = project();
    project.chains[0].steps[0].follow = Some(shelloop::FollowAction::Next);
    assert!(project.validate().is_err());
}
