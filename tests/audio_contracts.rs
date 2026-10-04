use shelloop::{sanitize_sample, select_named_device_index, write_mono_interleaved};

#[test]
fn mono_samples_are_duplicated_across_output_channels() {
    let mut output = [0.0_f32; 6];
    let written = write_mono_interleaved(&mut output, 2, &[0.25, -0.5, 0.75]).unwrap();

    assert_eq!(written, 3);
    assert_eq!(output, [0.25, 0.25, -0.5, -0.5, 0.75, 0.75]);
}

#[test]
fn audio_samples_are_clipped_and_non_finite_values_become_silence() {
    assert_eq!(sanitize_sample(2.0), 1.0);
    assert_eq!(sanitize_sample(-2.0), -1.0);
    assert_eq!(sanitize_sample(f32::NAN), 0.0);
    assert_eq!(sanitize_sample(f32::INFINITY), 0.0);
}

#[test]
fn interleaved_writer_rejects_invalid_channel_counts_and_short_buffers() {
    let mut empty = [];
    assert!(write_mono_interleaved(&mut empty, 0, &[0.0]).is_err());

    let mut short = [0.0_f32; 3];
    assert!(write_mono_interleaved(&mut short, 2, &[0.0, 0.0]).is_err());
}

#[test]
fn requested_audio_device_selection_is_exact_and_case_insensitive() {
    let names = vec![
        "Built-in Audio".to_string(),
        "Focusrite USB".to_string(),
        "HDMI".to_string(),
    ];

    assert_eq!(select_named_device_index(&names, "focusrite usb").unwrap(), 1);
    assert!(select_named_device_index(&names, "missing device").is_err());
}
