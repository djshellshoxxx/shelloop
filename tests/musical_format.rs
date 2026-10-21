//! File-format guarantees for the musical additions: existing pattern JSON
//! loads and re-serialises unchanged, new fields are omitted when default,
//! and the bundled example patterns validate.

use shelloop::{parse_pattern_json, Humanize, Pattern, PatternStep, StepChord, StepRatio};

fn legacy_json() -> &'static str {
    r#"{
  "name": "Legacy",
  "seed": 5,
  "swing": 0.0,
  "channel": 0,
  "steps": [
    {"note": 36, "velocity": 0.9, "gate": 0.5, "probability": 1.0, "ratchets": 1, "microtiming_frames": 0},
    null
  ]
}"#
}

#[test]
fn legacy_pattern_has_no_musical_keys_when_written_back() {
    let pattern = parse_pattern_json(legacy_json()).unwrap();
    assert!(pattern.chords.is_empty());
    assert!(pattern.ratios.is_empty());
    assert!(pattern.humanize.is_default());
    let written = serde_json::to_string(&pattern).unwrap();
    assert!(!written.contains("chords"), "{written}");
    assert!(!written.contains("ratios"), "{written}");
    assert!(!written.contains("humanize"), "{written}");
}

#[test]
fn musical_fields_round_trip() {
    let mut pattern = Pattern::new(
        "Musical",
        1,
        0.0,
        0,
        vec![
            Some(PatternStep {
                note: 48,
                velocity: 0.8,
                gate: 0.5,
                probability: 1.0,
                ratchets: 1,
                microtiming_frames: 0,
            }),
            None,
        ],
    )
    .unwrap();
    pattern.chords.push(StepChord {
        step: 0,
        notes: vec![51, 55],
    });
    pattern.ratios.push(StepRatio {
        step: 0,
        play: 1,
        of: 2,
    });
    pattern.humanize = Humanize {
        timing_frames: 96,
        velocity: 0.1,
    };
    let json = serde_json::to_string(&pattern).unwrap();
    let back = parse_pattern_json(&json).unwrap();
    assert_eq!(back, pattern);
}

#[test]
fn invalid_musical_data_is_rejected_on_load() {
    let base = legacy_json();
    let bad_chord = base.replace(
        "\"channel\": 0,",
        "\"channel\": 0, \"chords\": [{\"step\": 1, \"notes\": [40]}],",
    );
    assert!(
        parse_pattern_json(&bad_chord).is_err(),
        "chord on an empty step"
    );
    let bad_ratio = base.replace(
        "\"channel\": 0,",
        "\"channel\": 0, \"ratios\": [{\"step\": 0, \"play\": 3, \"of\": 2}],",
    );
    assert!(parse_pattern_json(&bad_ratio).is_err());
    let bad_humanize = base.replace(
        "\"channel\": 0,",
        "\"channel\": 0, \"humanize\": {\"timing_frames\": 0, \"velocity\": 0.9},",
    );
    assert!(parse_pattern_json(&bad_humanize).is_err());
}

#[test]
fn bundled_examples_validate() {
    let root = concat!(env!("CARGO_MANIFEST_DIR"), "/patterns/");
    for name in ["example-euclid-minor7.json", "example-pentatonic-arp.json"] {
        let text = std::fs::read_to_string(format!("{root}{name}")).unwrap();
        let pattern = parse_pattern_json(&text).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert!(
            pattern.steps.iter().any(Option::is_some),
            "{name} has notes"
        );
    }
    let chords = parse_pattern_json(
        &std::fs::read_to_string(format!("{root}example-euclid-minor7.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(chords.chords.len(), 2);
    assert_eq!(chords.ratios.len(), 1);
}
