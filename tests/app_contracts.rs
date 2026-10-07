use shelloop::{parse_startup_options, MidiPortSelector, StartupOptions};

#[test]
fn startup_options_default_to_hardware_auto_selection() {
    let options = parse_startup_options(std::iter::empty::<&str>()).unwrap();
    assert_eq!(options, StartupOptions::default());
}

#[test]
fn startup_options_parse_audio_midi_and_runtime_flags() {
    let options = parse_startup_options([
        "--audio-device",
        "Focusrite USB",
        "--midi-name",
        "Launchkey MIDI",
        "--polyphony",
        "24",
        "--list-devices",
    ])
    .unwrap();

    assert_eq!(options.audio_device.as_deref(), Some("Focusrite USB"));
    assert_eq!(
        options.midi_port,
        Some(MidiPortSelector::Name("Launchkey MIDI".to_string()))
    );
    assert_eq!(options.polyphony, 24);
    assert!(options.list_devices);
}

#[test]
fn startup_options_support_mouse_xy_performance_mode() {
    let options = parse_startup_options(["--mouse-xy"]).unwrap();
    assert!(options.mouse_xy);
}

#[test]
fn startup_options_support_wav_recording_path() {
    let options = parse_startup_options(["--record", "captures/demo.wav"]).unwrap();
    assert_eq!(options.record_path.as_deref(), Some("captures/demo.wav"));
}

#[test]
fn startup_options_reject_missing_or_empty_recording_path() {
    assert!(parse_startup_options(["--record"]).is_err());
    assert!(parse_startup_options(["--record", ""]).is_err());
}

#[test]
fn startup_options_support_live_pattern_playback() {
    let options = parse_startup_options([
        "--pattern",
        "patterns/four-on-floor.json",
        "--bpm",
        "138",
        "--steps-per-beat",
        "4",
    ])
    .unwrap();

    assert_eq!(
        options.pattern_path.as_deref(),
        Some("patterns/four-on-floor.json")
    );
    assert_eq!(options.bpm, 138);
    assert_eq!(options.steps_per_beat, 4);
}

#[test]
fn startup_options_support_midi_index_and_no_midi() {
    let by_index = parse_startup_options(["--midi-index", "2"]).unwrap();
    assert_eq!(by_index.midi_port, Some(MidiPortSelector::Index(2)));

    let disabled = parse_startup_options(["--no-midi"]).unwrap();
    assert!(disabled.no_midi);
}

#[test]
fn startup_options_reject_conflicts_unknown_flags_and_bad_ranges() {
    assert!(parse_startup_options(["--midi-index", "1", "--midi-name", "keys"]).is_err());
    assert!(parse_startup_options(["--polyphony", "0"]).is_err());
    assert!(parse_startup_options(["--polyphony", "257"]).is_err());
    assert!(parse_startup_options(["--bpm", "19"]).is_err());
    assert!(parse_startup_options(["--bpm", "401"]).is_err());
    assert!(parse_startup_options(["--steps-per-beat", "0"]).is_err());
    assert!(parse_startup_options(["--steps-per-beat", "65"]).is_err());
    assert!(parse_startup_options(["--pattern", ""]).is_err());
    assert!(parse_startup_options(["--unknown"]).is_err());
    assert!(parse_startup_options(["--audio-device"]).is_err());
}

#[test]
fn startup_options_support_pc_speaker_probe() {
    let options = parse_startup_options([
        "--pc-speaker-test",
        "--pc-speaker-frequency",
        "880",
        "--pc-speaker-duration",
        "300",
    ])
    .unwrap();

    assert!(options.pc_speaker_test);
    assert_eq!(options.pc_speaker_frequency_hz, 880);
    assert_eq!(options.pc_speaker_duration_ms, 300);
}

#[test]
fn startup_options_reject_invalid_pc_speaker_probe_values() {
    assert!(parse_startup_options(["--pc-speaker-frequency", "440"]).is_err());
    assert!(parse_startup_options(["--pc-speaker-duration", "100"]).is_err());
    assert!(parse_startup_options(["--pc-speaker-test", "--pc-speaker-frequency", "36"]).is_err());
    assert!(
        parse_startup_options(["--pc-speaker-test", "--pc-speaker-frequency", "32768"]).is_err()
    );
    assert!(parse_startup_options(["--pc-speaker-test", "--pc-speaker-duration", "0"]).is_err());
    assert!(parse_startup_options(["--pc-speaker-test", "--pc-speaker-duration", "5001"]).is_err());
}

#[test]
fn startup_options_support_multitrack_project_and_reject_pattern_conflict() {
    let options = parse_startup_options(["--project", "sets/live.json"]).unwrap();
    assert_eq!(options.project_path.as_deref(), Some("sets/live.json"));

    assert!(parse_startup_options([
        "--project",
        "sets/live.json",
        "--pattern",
        "patterns/bass.json",
    ])
    .is_err());
}

#[test]
fn waveform_is_off_by_default_and_can_start_visible_in_either_style() {
    let defaults = parse_startup_options(Vec::<String>::new()).unwrap();
    assert!(!defaults.waveform);
    assert_eq!(defaults.waveform_style, shelloop::WaveformStyle::Scope);

    let options = parse_startup_options(["--waveform", "--waveform-style", "history"]).unwrap();
    assert!(options.waveform);
    assert_eq!(options.waveform_style, shelloop::WaveformStyle::History);
}

#[test]
fn waveform_style_requires_a_known_value() {
    assert!(parse_startup_options(["--waveform-style"]).is_err());
    assert!(parse_startup_options(["--waveform-style", "bars"]).is_err());
}
