//! End-to-end musical commands through `SessionController::execute`, with the
//! audio engine applying every message (same harness as tests/controller.rs).

use crossbeam_channel::{bounded, Receiver};
use shelloop::{
    apply_audio_message, AudioMessage, Garbage, LibraryPattern, MultiTrackEngine,
    MultiTrackProject, Pattern, PatternId, PatternStep, SessionController, TrackDefinition,
    TrackId, TrackKind,
};

const RATE: u32 = 48_000;

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

fn project() -> MultiTrackProject {
    let pattern = Pattern::new("p", 1, 0.0, 0, vec![step(48), None, None, None]).unwrap();
    let mut bass = TrackDefinition::new(TrackId(1), "bass", TrackKind::Synth, pattern.clone());
    bass.pattern_library = vec![LibraryPattern {
        id: PatternId(1),
        pattern: pattern.clone(),
    }];
    MultiTrackProject::new(5, 120.0, 4, vec![bass])
}

struct Rig {
    controller: SessionController,
    receiver: Receiver<AudioMessage>,
    engine: MultiTrackEngine,
    garbage: crossbeam_channel::Sender<Garbage>,
}

impl Rig {
    fn new() -> Self {
        Self::with_path(None)
    }

    fn with_path(path: Option<std::path::PathBuf>) -> Self {
        let project = project();
        let (sender, receiver) = bounded(64);
        let engine = MultiTrackEngine::from_project(RATE, 4, &project, None).unwrap();
        let controller = SessionController::new(project, path, RATE, sender).unwrap();
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
        self.apply_pending();
        result
    }

    fn apply_pending(&mut self) {
        while let Ok(message) = self.receiver.try_recv() {
            apply_audio_message(&mut self.engine, message, &self.garbage);
        }
    }

    fn pattern(&self) -> &Pattern {
        &self.controller.project().tracks[0].pattern
    }

    fn render(&mut self, frames: u64) -> f32 {
        let mut peak = 0.0_f32;
        for _ in 0..frames {
            let (left, right) = self.engine.next_stereo_frame();
            peak = peak.max(left.abs()).max(right.abs());
        }
        peak
    }
}

fn steps_notes(pattern: &Pattern) -> Vec<Option<u8>> {
    pattern
        .steps
        .iter()
        .map(|step| step.map(|s| s.note))
        .collect()
}

#[test]
fn scale_snaps_typed_notes_and_applies_to_the_pattern() {
    let mut rig = Rig::new();
    rig.run("scale C major").unwrap();
    rig.run("step 2 note 61").unwrap(); // C# snaps down to C
    assert_eq!(rig.pattern().steps[1].unwrap().note, 60);
    rig.run("scale off").unwrap();
    rig.run("step 3 note 61").unwrap();
    assert_eq!(rig.pattern().steps[2].unwrap().note, 61);
    rig.run("scale F# minor").unwrap();
    rig.run("scale apply").unwrap();
    assert!(rig.pattern().steps.iter().flatten().all(|step| {
        let pitch_class = (step.note + 12 - 6) % 12;
        [0, 2, 3, 5, 7, 8, 10].contains(&pitch_class)
    }));
    assert!(rig.run("scale snap maybe").is_err());
    assert!(rig.run("scale list").unwrap().contains("dorian"));
}

#[test]
fn chord_and_arp_write_undoable_steps() {
    let mut rig = Rig::new();
    rig.run("chord C4 maj").unwrap();
    assert_eq!(rig.pattern().steps[0].unwrap().note, 60);
    assert_eq!(rig.pattern().chords.len(), 1);
    assert_eq!(rig.pattern().chords[0].notes, vec![64, 67]);
    // `step 2 toggle` selects index 1, so the arpeggio starts there, every
    // two steps, through the minor triad.
    rig.run("step 2 toggle").unwrap();
    rig.run("arp C4 min up 1 2").unwrap();
    assert_eq!(
        steps_notes(rig.pattern()),
        [Some(60), Some(60), None, Some(63)]
    );
    // The chord on step 1 lies outside the arpeggio's range only if it was
    // not overwritten; here step 0 keeps its chord untouched.
    assert_eq!(rig.pattern().chords.len(), 1);
    rig.run("undo").unwrap();
    assert_eq!(steps_notes(rig.pattern()), [Some(60), Some(60), None, None]);
    assert!(rig.run("chord C4 maj 9").is_err());
}

#[test]
fn euclid_rhythm_then_undo_restores_the_previous_pattern() {
    let mut rig = Rig::new();
    let before = rig.pattern().clone();
    rig.run("euclid 3 8 note 36").unwrap();
    let onsets: Vec<usize> = rig
        .pattern()
        .steps
        .iter()
        .enumerate()
        .filter_map(|(index, step)| step.map(|_| index))
        .collect();
    assert_eq!(rig.pattern().steps.len(), 8);
    assert_eq!(onsets, [0, 3, 6]);
    rig.run("undo").unwrap();
    assert_eq!(rig.pattern().steps, before.steps);
    assert!(rig.run("euclid 9 8").is_err());
    assert!(rig.run("euclid 3 8 bogus 1").is_err());
}

#[test]
fn humanize_ratio_and_clipboard_commands() {
    let mut rig = Rig::new();
    rig.run("humanize 5 0.1").unwrap();
    assert_eq!(rig.pattern().humanize.timing_frames, 240);
    assert!((rig.pattern().humanize.velocity - 0.1).abs() < 1e-6);
    rig.run("humanize off").unwrap();
    assert!(rig.pattern().humanize.is_default());

    rig.run("ratio 1 1:2").unwrap();
    assert_eq!(rig.pattern().ratios.len(), 1);
    assert!(rig.run("ratio 2 1:2").is_err(), "step 2 has no note");
    rig.run("ratio 1 off").unwrap();
    assert!(rig.pattern().ratios.is_empty());

    rig.run("pattern copy").unwrap();
    rig.run("pattern clear").unwrap();
    assert!(rig.pattern().steps.iter().all(Option::is_none));
    rig.run("pattern paste").unwrap();
    assert_eq!(steps_notes(rig.pattern())[0], Some(48));

    rig.run("pattern transpose 5").unwrap();
    assert_eq!(steps_notes(rig.pattern())[0], Some(53));
    assert!(rig.run("pattern transpose 49").is_err());
    rig.run("pattern transpose -5").unwrap();
    assert_eq!(steps_notes(rig.pattern())[0], Some(48));
}

#[test]
fn chords_follow_rotation_and_shrink_safely() {
    let mut rig = Rig::new();
    rig.run("step 4 toggle").unwrap(); // select index 3
    rig.run("chord C4 maj").unwrap();
    assert_eq!(rig.pattern().chords[0].step, 3);
    rig.run("rotate right 1").unwrap();
    assert_eq!(rig.pattern().chords[0].step, 0, "chord moves with its step");
    assert_eq!(rig.pattern().steps[0].unwrap().note, 60);
    // Shrinking past a chord must not fail validation; the chord goes with its step.
    rig.run("step 2 toggle").unwrap(); // index 1
    rig.run("chord C4 maj").unwrap();
    assert_eq!(rig.pattern().chords.len(), 2);
    rig.run("length 1").unwrap();
    let steps: Vec<u16> = rig
        .pattern()
        .chords
        .iter()
        .map(|chord| chord.step)
        .collect();
    assert_eq!(steps, [0], "only the chord on the surviving step remains");
    rig.run("undo").unwrap();
    assert_eq!(rig.pattern().chords.len(), 2, "undo restores the chord");
    // Clearing the step that carries a chord drops the chord with it.
    rig.run("step 2 clear").unwrap();
    assert_eq!(rig.pattern().chords.len(), 1);
}

#[test]
fn paste_without_copy_is_an_error_and_pattern_id_still_selects() {
    let mut fresh = Rig::new();
    assert!(fresh.run("pattern paste").is_err());
    assert!(
        fresh.run("pattern 1").is_ok(),
        "numeric pattern selection is unchanged"
    );
}

#[test]
fn tap_tempo_retimes_the_engine() {
    let mut rig = Rig::new();
    rig.controller.tick(1_000);
    assert_eq!(rig.run("tap").unwrap(), "tap again to measure the tempo");
    rig.controller.tick(1_500);
    let message = rig.run("tap").unwrap();
    assert!(message.contains("120.00"), "{message}");
    assert_eq!(rig.engine.bpm(), 120.0);
    // A pause longer than the timeout starts a fresh measurement.
    rig.controller.tick(10_000);
    rig.run("tap").unwrap();
    rig.controller.tick(10_250);
    rig.run("tap").unwrap();
    // Taps 250 ms apart are 240 BPM.
    assert_eq!(rig.engine.bpm(), 240.0);
    assert_eq!(rig.controller.project().bpm, 240.0);
}

#[test]
fn count_in_clicks_then_starts_the_transport() {
    let mut rig = Rig::new();
    rig.run("pause").unwrap();
    assert!(!rig.engine.is_playing());
    rig.run("countin 1").unwrap();
    rig.run("metronome on 1").unwrap();
    assert!(rig.run("play").unwrap().contains("count-in"));
    // One bar at 120 BPM is 2 seconds: 96_000 frames of clicks, silent pattern.
    let peak = rig.render(96_000 - 10);
    assert!(
        peak > 0.05,
        "count-in clicks should be audible, peak {peak}"
    );
    assert!(
        !rig.engine.is_playing(),
        "transport waits for the last beat"
    );
    rig.render(10);
    assert!(
        rig.engine.is_playing(),
        "transport starts after the count-in"
    );
    rig.run("pause").unwrap();
    assert!(!rig.engine.is_playing());
    assert!(rig.run("countin 9").is_err());
}

#[test]
fn metronome_clicks_only_while_enabled() {
    let mut rig = Rig::new();
    // Silence the pattern so only the click can make sound.
    rig.run("step 1 clear").unwrap();
    rig.run("metronome on 1").unwrap();
    assert!(rig.engine.is_playing());
    let clicked = rig.render(2_000);
    assert!(clicked > 0.05, "beat one should click, peak {clicked}");
    rig.run("metronome off").unwrap();
    // Two more beats at 120 BPM pass with the metronome disabled.
    assert_eq!(rig.render(48_000), 0.0);
}

#[test]
fn autosave_writes_a_sidecar_and_leaves_the_project_alone() {
    let dir = tempfile::tempdir().unwrap();
    let project_path = dir.path().join("song.json");
    let mut rig = Rig::with_path(Some(project_path.clone()));
    rig.run("step 2 toggle").unwrap();
    assert!(rig.controller.tick(59_999).is_none());
    let message = rig.controller.tick(60_000).expect("autosave is due");
    assert!(message.starts_with("autosaved to"), "{message}");
    let sidecar = dir.path().join("song.autosave.json");
    assert!(sidecar.exists());
    assert!(
        !project_path.exists(),
        "project file is only written by save"
    );
    let text = std::fs::read_to_string(&sidecar).unwrap();
    let restored: MultiTrackProject = serde_json::from_str(&text).unwrap();
    assert_eq!(
        restored.tracks[0].pattern.steps[1].map(|s| s.note),
        Some(60)
    );

    rig.run("autosave off").unwrap();
    rig.run("step 3 toggle").unwrap();
    assert!(rig.controller.tick(500_000).is_none());
    assert!(
        rig.run("autosave every 1").is_err(),
        "interval has a 5 second floor"
    );
    assert!(rig.run("autosave").unwrap().contains("autosave off"));
}

#[test]
fn autosave_needs_a_project_path() {
    let mut rig = Rig::new();
    rig.run("step 2 toggle").unwrap();
    assert!(rig.controller.tick(120_000).is_none());
    assert!(rig
        .run("autosave")
        .unwrap()
        .contains("needs a project path"));
}

#[test]
fn help_lists_the_new_commands() {
    let mut rig = Rig::new();
    let help = rig.run("help").unwrap();
    for word in [
        "scale",
        "chord",
        "arp",
        "euclid",
        "humanize",
        "ratio",
        "tap",
        "metronome",
        "countin",
        "autosave",
        "pattern copy",
    ] {
        assert!(help.contains(word), "help is missing {word}");
    }
}
