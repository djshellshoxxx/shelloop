use shelloop::{
    create_sample_renderer, sanitize_sample, select_named_device_index, write_mono_interleaved,
    write_stereo_interleaved,
};

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

    assert_eq!(
        select_named_device_index(&names, "focusrite usb").unwrap(),
        1
    );
    assert!(select_named_device_index(&names, "missing device").is_err());
}

#[test]
fn renderer_factory_receives_the_device_sample_rate() {
    let renderer = create_sample_renderer(48_000, |sample_rate| {
        Ok::<_, String>(move || sample_rate as f32)
    })
    .unwrap();

    assert_eq!(renderer(), 48_000.0);
    assert!(create_sample_renderer(0, |_| Ok::<_, String>(|| 0.0)).is_err());
}

#[test]
fn stereo_samples_preserve_left_and_right_on_two_channel_output() {
    let mut output = [0.0_f32; 4];
    let written = write_stereo_interleaved(
        &mut output,
        2,
        &[(0.25, -0.5), (0.75, -0.25)],
    )
    .unwrap();

    assert_eq!(written, 2);
    assert_eq!(output, [0.25, -0.5, 0.75, -0.25]);
}

#[test]
fn stereo_writer_downmixes_for_mono_and_silences_extra_channels() {
    let mut mono = [0.0_f32; 2];
    write_stereo_interleaved(&mut mono, 1, &[(1.0, 0.0), (-1.0, 1.0)]).unwrap();
    assert_eq!(mono, [0.5, 0.0]);

    let mut surround = [99.0_f32; 4];
    write_stereo_interleaved(&mut surround, 4, &[(0.25, -0.5)]).unwrap();
    assert_eq!(surround, [0.25, -0.5, 0.0, 0.0]);
}

#[test]
fn stereo_writer_sanitizes_each_channel_and_rejects_short_buffers() {
    let mut output = [0.0_f32; 2];
    write_stereo_interleaved(&mut output, 2, &[(f32::NAN, 2.0)]).unwrap();
    assert_eq!(output, [0.0, 1.0]);

    let mut short = [0.0_f32; 3];
    assert!(write_stereo_interleaved(&mut short, 2, &[(0.0, 0.0), (0.0, 0.0)]).is_err());
    assert!(write_stereo_interleaved(&mut [], 0, &[(0.0, 0.0)]).is_err());
}
