use shelloop::{presets::PresetStore, Oscillator, SynthPatch};

#[test]
fn named_presets_round_trip_without_clobbering_or_escaping_the_directory() {
    let directory = tempfile::tempdir().unwrap();
    let store = PresetStore::new(directory.path().join("presets"));
    assert!(store.list().unwrap().is_empty());
    let mut patch = SynthPatch::legacy(Oscillator::Saw);
    patch.output_gain = 0.42;
    store.save("Warm_Pad-1", patch, 48_000.0).unwrap();
    assert_eq!(store.list().unwrap(), ["Warm_Pad-1"]);
    assert_eq!(store.load("Warm_Pad-1", 48_000.0).unwrap(), patch);
    assert!(store.save("Warm_Pad-1", patch, 48_000.0).is_err());
    for name in ["../escape", "a/b", "a\\b", "", ".", "hidden.json"] {
        assert!(store.save(name, patch, 48_000.0).is_err());
        assert!(store.load(name, 48_000.0).is_err());
        assert!(store.delete(name).is_err());
    }
    assert_eq!(store.load("Warm_Pad-1", 48_000.0).unwrap(), patch);
    store.delete("Warm_Pad-1").unwrap();
    assert!(store.list().unwrap().is_empty());
}

#[test]
fn invalid_or_future_presets_cannot_enter_the_live_engine() {
    let directory = tempfile::tempdir().unwrap();
    let store = PresetStore::new(directory.path());
    let mut patch = SynthPatch::legacy(Oscillator::Sine);
    patch.output_gain = f32::NAN;
    assert!(store.save("Invalid", patch, 48_000.0).is_err());
    assert!(!directory.path().join("Invalid.json").exists());
    std::fs::write(
        directory.path().join("Future.json"),
        r#"{"schema_version":999,"name":"Future","patch":{}}"#,
    )
    .unwrap();
    assert!(store.load("Future", 48_000.0).is_err());
    std::fs::write(directory.path().join("Huge.json"), vec![b' '; 65_537]).unwrap();
    assert!(store.load("Huge", 48_000.0).is_err());
}
