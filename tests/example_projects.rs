//! Every bundled example project loads, validates, builds an engine at
//! several sample rates and renders finite audio.

use shelloop::{MultiTrackEngine, MultiTrackProject, SampleContext, DEFAULT_SAMPLE_MEMORY_BUDGET};
use std::path::Path;

fn render(name: &str, rate: u32, seconds: u32) -> (f32, usize) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("projects")
        .join(name);
    let project = MultiTrackProject::load(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
    let base_dir = path.parent().unwrap();
    let bank = project
        .load_sample_assets(base_dir, DEFAULT_SAMPLE_MEMORY_BUDGET)
        .unwrap();
    let mut engine = MultiTrackEngine::from_project(
        rate,
        16,
        &project,
        Some(SampleContext {
            assets: &bank,
            base_dir,
        }),
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut peak = 0.0_f32;
    for _ in 0..(rate * seconds) {
        let (l, r) = engine.next_stereo_frame();
        assert!(l.is_finite() && r.is_finite(), "{name}");
        peak = peak.max(l.abs()).max(r.abs());
    }
    (peak, project.tracks.len())
}

#[test]
fn all_bundled_projects_render_at_every_supported_rate() {
    for name in [
        "example-multitrack.json",
        "example-synth-patches.json",
        "example-samples.json",
        "example-performance.json",
        "example-sound-design.json",
        "example-sliced-break.json",
    ] {
        for rate in [44_100, 48_000, 96_000] {
            let (peak, tracks) = render(name, rate, 2);
            assert!(tracks >= 2, "{name}");
            assert!(peak > 0.02, "{name} @ {rate} is silent (peak {peak})");
            assert!(peak < 4.0, "{name} @ {rate} is unreasonably loud ({peak})");
        }
    }
}

#[test]
fn new_examples_exercise_the_new_features() {
    let load = |name: &str| {
        MultiTrackProject::load(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("projects")
                .join(name),
        )
        .unwrap()
    };
    let design = load("example-sound-design.json");
    let routes: usize = design
        .tracks
        .iter()
        .filter_map(|t| t.synth_patch)
        .map(|p| p.modulation.routes().len())
        .sum();
    assert!(routes >= 8);
    assert!(design.tracks.iter().any(|t| !t.inserts.is_empty()));
    assert_eq!(design.effects.master.len(), 1);

    let sliced = load("example-sliced-break.json");
    let sample = sliced.tracks[0].sample.as_ref().unwrap();
    assert_eq!(sample.slices.regions.len(), 8);
    assert!(!sample.sample_shaping.is_default());

    // Round trip keeps the new blocks byte-for-byte meaningful.
    let again: MultiTrackProject =
        serde_json::from_str(&serde_json::to_string(&design).unwrap()).unwrap();
    assert_eq!(again, design);
    let again: MultiTrackProject =
        serde_json::from_str(&serde_json::to_string(&sliced).unwrap()).unwrap();
    assert_eq!(again, sliced);
}
