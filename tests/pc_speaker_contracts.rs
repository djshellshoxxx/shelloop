use shelloop::{pc_speaker_backend, pc_speaker_test, PcSpeakerBackend};

#[test]
fn native_platforms_report_their_pc_speaker_capability() {
    #[cfg(target_os = "linux")]
    assert_eq!(pc_speaker_backend(), PcSpeakerBackend::LinuxConsoleSpeaker);

    #[cfg(windows)]
    assert_eq!(
        pc_speaker_backend(),
        PcSpeakerBackend::WindowsBeepCompatibility
    );

    #[cfg(not(any(target_os = "linux", windows)))]
    assert_eq!(pc_speaker_backend(), PcSpeakerBackend::Unsupported);
}

#[test]
fn pc_speaker_probe_rejects_out_of_range_values_before_hardware_access() {
    assert!(pc_speaker_test(36, 250).is_err());
    assert!(pc_speaker_test(32_768, 250).is_err());
    assert!(pc_speaker_test(440, 0).is_err());
    assert!(pc_speaker_test(440, 5_001).is_err());
}
