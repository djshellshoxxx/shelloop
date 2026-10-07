use shelloop::{pc_speaker_backend, pc_speaker_test, PcSpeakerBackend};

#[test]
fn supported_ci_platforms_report_a_pc_speaker_backend() {
    #[cfg(target_os = "linux")]
    assert_eq!(pc_speaker_backend(), PcSpeakerBackend::LinuxConsoleSpeaker);

    #[cfg(windows)]
    assert_eq!(
        pc_speaker_backend(),
        PcSpeakerBackend::WindowsBeepCompatibility
    );
}

#[test]
fn pc_speaker_probe_rejects_out_of_range_values_before_hardware_access() {
    assert!(pc_speaker_test(36, 250).is_err());
    assert!(pc_speaker_test(32_768, 250).is_err());
    assert!(pc_speaker_test(440, 0).is_err());
    assert!(pc_speaker_test(440, 5_001).is_err());
}
