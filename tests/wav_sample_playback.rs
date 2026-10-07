use shelloop::sample::{BYTES_PER_FRAME, MAX_PLAYBACK_INCREMENT};
use shelloop::{
    decode_wav_file, decode_wav_reader, CompiledSamplePlayback, DecodedWav, EngineCommand,
    MultiTrackEngine, MultiTrackProject, Pattern, PatternStep, RealtimeSampler, SampleAsset,
    SampleAssetBank, SampleAssetId, SampleContext, SampleMode, SampleSettings, StereoFrame,
    TrackDefinition, TrackId, TrackKind, DEFAULT_SAMPLE_MEMORY_BUDGET,
};
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MIB: usize = 1024 * 1024;

fn wav_bytes(
    spec: hound::WavSpec,
    write: impl FnOnce(&mut hound::WavWriter<&mut Cursor<Vec<u8>>>),
) -> Vec<u8> {
    let mut cursor = Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
        write(&mut writer);
        writer.finalize().unwrap();
    }
    cursor.into_inner()
}

fn int_spec(channels: u16, bits: u16) -> hound::WavSpec {
    hound::WavSpec {
        channels,
        sample_rate: 48_000,
        bits_per_sample: bits,
        sample_format: hound::SampleFormat::Int,
    }
}

fn write_wav(
    path: &Path,
    sample_rate: u32,
    channels: u16,
    frames: usize,
    value: impl Fn(usize) -> f32,
) {
    let spec = hound::WavSpec {
        channels,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for frame in 0..frames {
        for _ in 0..channels {
            writer.write_sample(value(frame)).unwrap();
        }
    }
    writer.finalize().unwrap();
}

/// An asset whose frame N holds the value N, so rendered output reveals
/// exactly which source frames were read.
fn ramp_asset(frames: usize, sample_rate: u32) -> SampleAsset {
    SampleAsset {
        id: SampleAssetId(0),
        source_path: PathBuf::from("ramp.wav"),
        source_sample_rate: sample_rate,
        channels: 1,
        frames: (0..frames)
            .map(|index| StereoFrame::new(index as f32, index as f32))
            .collect::<Vec<_>>()
            .into(),
    }
}

fn constant_asset(frames: usize, sample_rate: u32) -> SampleAsset {
    SampleAsset {
        id: SampleAssetId(0),
        source_path: PathBuf::from("dc.wav"),
        source_sample_rate: sample_rate,
        channels: 1,
        frames: vec![StereoFrame::new(0.5, 0.5); frames].into(),
    }
}

fn settings(mode: SampleMode) -> SampleSettings {
    let mut settings = SampleSettings::new("ramp.wav", mode);
    settings.release_secs = 0.0;
    settings
}

fn note_on(note: u8) -> EngineCommand {
    EngineCommand::NoteOn {
        channel: 0,
        note,
        velocity: 1.0,
    }
}

fn note_off(note: u8) -> EngineCommand {
    EngineCommand::NoteOff { channel: 0, note }
}

/// Render until the sampler falls silent and return the number of frames.
fn frames_until_silent(sampler: &mut RealtimeSampler, limit: usize) -> usize {
    for frame in 0..limit {
        sampler.next_stereo_frame();
        if sampler.active_voice_count() == 0 {
            return frame + 1;
        }
    }
    limit
}

fn render_left(sampler: &mut RealtimeSampler, frames: usize) -> Vec<f32> {
    (0..frames).map(|_| sampler.next_stereo_frame().0).collect()
}

#[test]
fn decodes_16_24_and_32_bit_pcm_and_float_to_expected_f32() {
    let pcm16 = wav_bytes(int_spec(1, 16), |w| {
        for value in [0_i32, 16_384, -32_768, 32_767] {
            w.write_sample(value as i16).unwrap();
        }
    });
    let decoded = decode_wav_reader(Cursor::new(pcm16), usize::MAX).unwrap();
    let left: Vec<f32> = decoded.frames.iter().map(|frame| frame.left).collect();
    assert_eq!(left[0], 0.0);
    assert_eq!(left[1], 0.5);
    assert_eq!(left[2], -1.0);
    assert!((left[3] - 32_767.0 / 32_768.0).abs() < 1e-6);

    let pcm24 = wav_bytes(int_spec(1, 24), |w| {
        w.write_sample(4_194_304_i32).unwrap();
        w.write_sample(-8_388_608_i32).unwrap();
    });
    let decoded = decode_wav_reader(Cursor::new(pcm24), usize::MAX).unwrap();
    assert_eq!(decoded.frames[0].left, 0.5);
    assert_eq!(decoded.frames[1].left, -1.0);

    let pcm32 = wav_bytes(int_spec(1, 32), |w| {
        w.write_sample(1_073_741_824_i32).unwrap();
        w.write_sample(i32::MIN).unwrap();
    });
    let decoded = decode_wav_reader(Cursor::new(pcm32), usize::MAX).unwrap();
    assert_eq!(decoded.frames[0].left, 0.5);
    assert_eq!(decoded.frames[1].left, -1.0);

    let float = wav_bytes(
        hound::WavSpec {
            channels: 1,
            sample_rate: 44_100,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
        |w| {
            for value in [0.25_f32, -0.75, f32::NAN] {
                w.write_sample(value).unwrap();
            }
        },
    );
    let decoded = decode_wav_reader(Cursor::new(float), usize::MAX).unwrap();
    assert_eq!(decoded.sample_rate, 44_100);
    assert_eq!(decoded.frames[0].left, 0.25);
    assert_eq!(decoded.frames[1].left, -0.75);
    assert_eq!(decoded.frames[2].left, 0.0, "non-finite input is silenced");
}

#[test]
fn mono_is_duplicated_and_stereo_keeps_channels() {
    let mono = wav_bytes(int_spec(1, 16), |w| w.write_sample(16_384_i16).unwrap());
    let decoded = decode_wav_reader(Cursor::new(mono), usize::MAX).unwrap();
    assert_eq!(decoded.channels, 1);
    assert_eq!(decoded.frames, vec![StereoFrame::new(0.5, 0.5)]);

    let stereo = wav_bytes(int_spec(2, 16), |w| {
        w.write_sample(16_384_i16).unwrap();
        w.write_sample(-16_384_i16).unwrap();
    });
    let decoded = decode_wav_reader(Cursor::new(stereo), usize::MAX).unwrap();
    assert_eq!(decoded.channels, 2);
    assert_eq!(decoded.frames, vec![StereoFrame::new(0.5, -0.5)]);
}

#[test]
fn unsupported_and_corrupt_wav_fail_before_activation() {
    assert!(decode_wav_reader(Cursor::new(b"not a wav file".to_vec()), usize::MAX).is_err());

    let surround = wav_bytes(int_spec(6, 16), |w| {
        for _ in 0..6 {
            w.write_sample(0_i16).unwrap();
        }
    });
    let error = decode_wav_reader(Cursor::new(surround), usize::MAX).unwrap_err();
    assert!(error.contains("channel count 6"), "{error}");

    let empty = wav_bytes(int_spec(1, 16), |_| {});
    assert!(decode_wav_reader(Cursor::new(empty), usize::MAX)
        .unwrap_err()
        .contains("no audio frames"));

    let mut truncated = wav_bytes(int_spec(1, 16), |w| {
        for _ in 0..100 {
            w.write_sample(1_000_i16).unwrap();
        }
    });
    truncated.truncate(truncated.len() - 50);
    assert!(decode_wav_reader(Cursor::new(truncated), usize::MAX).is_err());
}

#[test]
fn source_rate_44_1k_plays_correct_duration_at_48k_output() {
    let asset = constant_asset(44_100, 44_100);
    let playback =
        CompiledSamplePlayback::new(&asset, &settings(SampleMode::OneShot), 48_000).unwrap();
    let mut sampler = RealtimeSampler::new(playback);
    sampler.handle(note_on(60));
    let frames = frames_until_silent(&mut sampler, 100_000);
    assert!(
        (47_999..=48_001).contains(&frames),
        "one second of 44.1 kHz audio rendered as {frames} frames at 48 kHz"
    );
}

#[test]
fn up_an_octave_halves_duration() {
    let asset = constant_asset(48_000, 48_000);
    let mut up = settings(SampleMode::OneShot);
    up.pitch_semitones = 12.0;
    let mut normal = RealtimeSampler::new(
        CompiledSamplePlayback::new(&asset, &settings(SampleMode::OneShot), 48_000).unwrap(),
    );
    let mut octave =
        RealtimeSampler::new(CompiledSamplePlayback::new(&asset, &up, 48_000).unwrap());
    normal.handle(note_on(60));
    octave.handle(note_on(60));
    let normal_frames = frames_until_silent(&mut normal, 100_000);
    let octave_frames = frames_until_silent(&mut octave, 100_000);
    assert_eq!(normal_frames, 48_000);
    assert!(
        (23_999..=24_001).contains(&octave_frames),
        "{octave_frames}"
    );
}

#[test]
fn root_note_transposes_chromatically_and_is_ignored_without_one() {
    let asset = constant_asset(48_000, 48_000);
    let mut chromatic = settings(SampleMode::OneShot);
    chromatic.root_note = Some(60);
    let playback = CompiledSamplePlayback::new(&asset, &chromatic, 48_000).unwrap();
    assert!((playback.increment_for_note(72) - 2.0).abs() < 1e-9);
    assert!((playback.increment_for_note(48) - 0.5).abs() < 1e-9);

    let drums =
        CompiledSamplePlayback::new(&asset, &settings(SampleMode::OneShot), 48_000).unwrap();
    assert_eq!(drums.increment_for_note(36), 1.0);
    assert_eq!(drums.increment_for_note(100), 1.0);
}

#[test]
fn forward_playback_reads_every_frame_of_the_region_once() {
    let asset = ramp_asset(100, 48_000);
    let mut region = settings(SampleMode::OneShot);
    region.start = 0.1;
    region.end = 0.2;
    let mut sampler =
        RealtimeSampler::new(CompiledSamplePlayback::new(&asset, &region, 48_000).unwrap());
    sampler.handle(note_on(60));
    let out = render_left(&mut sampler, 12);
    let expected: Vec<f32> = (10..20)
        .map(|value| value as f32)
        .chain([0.0, 0.0])
        .collect();
    assert_eq!(out, expected);
}

#[test]
fn reverse_starts_at_the_last_frame_and_stops_at_the_first_without_underflow() {
    let asset = ramp_asset(100, 48_000);
    let mut reverse = settings(SampleMode::OneShot);
    reverse.reverse = true;
    reverse.end = 0.05;
    let mut sampler =
        RealtimeSampler::new(CompiledSamplePlayback::new(&asset, &reverse, 48_000).unwrap());
    sampler.handle(note_on(60));
    let out = render_left(&mut sampler, 7);
    assert_eq!(out, vec![4.0, 3.0, 2.0, 1.0, 0.0, 0.0, 0.0]);
    assert_eq!(sampler.active_voice_count(), 0);

    // A fast reverse read must also stop cleanly instead of wrapping below zero.
    let mut fast = reverse.clone();
    fast.pitch_semitones = 48.0;
    let mut sampler =
        RealtimeSampler::new(CompiledSamplePlayback::new(&asset, &fast, 48_000).unwrap());
    sampler.handle(note_on(60));
    assert!(render_left(&mut sampler, 4)
        .iter()
        .all(|value| value.is_finite()));
    assert_eq!(sampler.active_voice_count(), 0);
}

#[test]
fn forward_loop_neither_doubles_nor_drops_a_frame_at_the_boundary() {
    let asset = ramp_asset(100, 48_000);
    let mut looped = settings(SampleMode::Loop);
    looped.start = 0.0;
    looped.loop_start = 0.02;
    looped.loop_end = 0.05;
    let playback = CompiledSamplePlayback::new(&asset, &looped, 48_000).unwrap();
    assert_eq!(playback.loop_region(), (2, 5));
    let mut sampler = RealtimeSampler::new(playback);
    sampler.handle(note_on(60));
    let out = render_left(&mut sampler, 11);
    assert_eq!(
        out,
        vec![0.0, 1.0, 2.0, 3.0, 4.0, 2.0, 3.0, 4.0, 2.0, 3.0, 4.0]
    );
}

#[test]
fn reverse_loop_wraps_from_loop_start_to_loop_end() {
    let asset = ramp_asset(10, 48_000);
    let mut looped = settings(SampleMode::Loop);
    looped.reverse = true;
    looped.loop_start = 0.2;
    looped.loop_end = 0.5;
    let mut sampler =
        RealtimeSampler::new(CompiledSamplePlayback::new(&asset, &looped, 48_000).unwrap());
    sampler.handle(note_on(60));
    let out = render_left(&mut sampler, 12);
    assert_eq!(
        out,
        vec![9.0, 8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 2.0, 4.0, 3.0, 2.0, 4.0]
    );
}

#[test]
fn loop_stops_after_note_off_and_release() {
    let asset = constant_asset(100, 48_000);
    let mut looped = settings(SampleMode::Loop);
    looped.release_secs = 10.0 / 48_000.0;
    let mut sampler =
        RealtimeSampler::new(CompiledSamplePlayback::new(&asset, &looped, 48_000).unwrap());
    sampler.handle(note_on(60));
    render_left(&mut sampler, 1_000);
    assert_eq!(sampler.active_voice_count(), 1, "loop sustains while held");
    sampler.handle(note_off(60));
    let tail = render_left(&mut sampler, 12);
    assert!(tail[0] > tail[5] && tail[5] > 0.0, "fades out: {tail:?}");
    assert_eq!(sampler.active_voice_count(), 0);
}

#[test]
fn one_shot_ignores_note_off_but_gate_obeys_it() {
    let asset = constant_asset(1_000, 48_000);
    let mut one_shot = RealtimeSampler::new(
        CompiledSamplePlayback::new(&asset, &settings(SampleMode::OneShot), 48_000).unwrap(),
    );
    one_shot.handle(note_on(60));
    render_left(&mut one_shot, 10);
    one_shot.handle(note_off(60));
    assert_eq!(frames_until_silent(&mut one_shot, 10_000), 990);

    let mut gate = RealtimeSampler::new(
        CompiledSamplePlayback::new(&asset, &settings(SampleMode::Gate), 48_000).unwrap(),
    );
    gate.handle(note_on(60));
    render_left(&mut gate, 10);
    gate.handle(note_off(60));
    assert_eq!(gate.active_voice_count(), 0, "zero release stops at once");
    assert_eq!(render_left(&mut gate, 1), vec![0.0]);
}

#[test]
fn attack_fades_in_from_silence() {
    let asset = constant_asset(1_000, 48_000);
    let mut soft = settings(SampleMode::OneShot);
    soft.attack_secs = 4.0 / 48_000.0;
    let mut sampler =
        RealtimeSampler::new(CompiledSamplePlayback::new(&asset, &soft, 48_000).unwrap());
    sampler.handle(note_on(60));
    let out = render_left(&mut sampler, 6);
    assert_eq!(out[0], 0.0);
    assert!(out[1] > 0.0 && out[1] < out[3]);
    assert_eq!(out[4], 0.5);
    assert_eq!(out[5], 0.5);
}

#[test]
fn voice_stealing_is_deterministic_released_first_then_oldest() {
    let asset = constant_asset(48_000, 48_000);
    let mut two_voices = settings(SampleMode::OneShot);
    two_voices.voices = 2;
    let playback = CompiledSamplePlayback::new(&asset, &two_voices, 48_000).unwrap();

    let run = || {
        let mut sampler = RealtimeSampler::new(playback.clone());
        sampler.handle(note_on(60));
        sampler.next_stereo_frame();
        sampler.handle(note_on(62));
        sampler.next_stereo_frame();
        // Note 62's key is released, so it is stolen before the older held 60.
        sampler.handle(note_off(62));
        sampler.handle(note_on(64));
        sampler.next_stereo_frame();
        let positions: Vec<f64> = sampler.voices().iter().map(|v| v.position()).collect();
        // Both voices still held now: the oldest (60, slot 0) is stolen.
        sampler.handle(note_on(65));
        let after: Vec<f64> = sampler.voices().iter().map(|v| v.position()).collect();
        (positions, after)
    };
    let (positions, after) = run();
    assert_eq!(
        positions,
        vec![3.0, 1.0],
        "slot 1 was restarted for note 64"
    );
    assert_eq!(
        after,
        vec![0.0, 1.0],
        "slot 0 (oldest held) was stolen for note 65"
    );
    assert_eq!(
        run(),
        (positions, after),
        "identical input gives identical stealing"
    );
}

#[test]
fn memory_budget_is_enforced_with_a_report() {
    let dir = tempfile::tempdir().unwrap();
    let small = dir.path().join("small.wav");
    let big = dir.path().join("big.wav");
    write_wav(&small, 48_000, 1, 100, |_| 0.1);
    write_wav(&big, 48_000, 2, 1_000, |_| 0.1);

    let mut bank = SampleAssetBank::new(500 * BYTES_PER_FRAME);
    bank.load_file(&small).unwrap();
    assert_eq!(bank.used_bytes(), 100 * BYTES_PER_FRAME);
    // Loading the same path again reuses the decoded asset.
    bank.load_file(&small).unwrap();
    assert_eq!(bank.len(), 1);

    let error = bank.load_file(&big).unwrap_err();
    assert!(error.contains("budget"), "{error}");
    assert!(
        error.contains(&(1_000 * BYTES_PER_FRAME).to_string()),
        "{error}"
    );
    assert_eq!(bank.len(), 1, "failed load leaves the bank unchanged");

    let decoded = DecodedWav {
        sample_rate: 48_000,
        channels: 1,
        frames: vec![StereoFrame::SILENT; 401],
    };
    assert!(bank.insert_decoded("other.wav", decoded).is_err());
    assert_eq!(DEFAULT_SAMPLE_MEMORY_BUDGET, 512 * MIB);
}

#[test]
fn rendering_stays_finite_at_maximum_pitch_and_rate() {
    let asset = SampleAsset {
        id: SampleAssetId(0),
        source_path: PathBuf::from("loud.wav"),
        source_sample_rate: 384_000,
        channels: 2,
        frames: vec![StereoFrame::new(1.0, -1.0); 4_096].into(),
    };
    for mode in [SampleMode::OneShot, SampleMode::Gate, SampleMode::Loop] {
        for reverse in [false, true] {
            let mut extreme = settings(mode);
            extreme.pitch_semitones = 48.0;
            extreme.fine_cents = 100.0;
            extreme.root_note = Some(0);
            extreme.reverse = reverse;
            extreme.gain = 2.0;
            let playback = CompiledSamplePlayback::new(&asset, &extreme, 8_000).unwrap();
            assert_eq!(playback.increment_for_note(127), MAX_PLAYBACK_INCREMENT);
            let mut sampler = RealtimeSampler::new(playback);
            for note in 0..=127 {
                sampler.handle(note_on(note));
            }
            for _ in 0..10_000 {
                let (left, right) = sampler.next_stereo_frame();
                assert!(left.is_finite() && right.is_finite());
            }
        }
    }
}

#[test]
fn settings_validation_rejects_bad_regions_and_ranges() {
    let base = SampleSettings::new("kick.wav", SampleMode::Loop);
    assert!(base.validate().is_ok());
    type Mutation = Box<dyn Fn(&mut SampleSettings)>;
    let cases: Vec<Mutation> = vec![
        Box::new(|s| s.path = " ".into()),
        Box::new(|s| s.start = 0.6),
        Box::new(|s| s.end = 1.5),
        Box::new(|s| {
            s.loop_start = 0.5;
            s.loop_end = 0.5
        }),
        Box::new(|s| {
            s.start = 0.3;
            s.loop_start = 0.1
        }),
        Box::new(|s| s.gain = f32::NAN),
        Box::new(|s| s.pan = 2.0),
        Box::new(|s| s.pitch_semitones = 49.0),
        Box::new(|s| s.fine_cents = -101.0),
        Box::new(|s| s.root_note = Some(128)),
        Box::new(|s| s.attack_secs = -0.1),
        Box::new(|s| s.release_secs = 1.5),
        Box::new(|s| s.voices = 0),
        Box::new(|s| s.voices = 65),
    ];
    for (index, mutate) in cases.iter().enumerate() {
        let mut invalid = base.clone();
        mutate(&mut invalid);
        assert!(
            invalid.validate().is_err(),
            "case {index} should be rejected"
        );
    }
}

#[test]
fn sample_settings_reject_unknown_fields_and_fill_defaults() {
    let parsed: SampleSettings =
        serde_json::from_str(r#"{"path": "kick.wav", "mode": "one_shot"}"#).unwrap();
    assert_eq!(parsed, SampleSettings::new("kick.wav", SampleMode::OneShot));
    assert!(serde_json::from_str::<SampleSettings>(
        r#"{"path": "kick.wav", "mode": "one_shot", "pitch": 3}"#
    )
    .is_err());
}

fn kick_pattern() -> Pattern {
    let hit = PatternStep {
        note: 36,
        velocity: 1.0,
        gate: 0.5,
        probability: 1.0,
        ratchets: 1,
        microtiming_frames: 0,
    };
    Pattern::new("Kick", 1, 0.0, 0, vec![Some(hit), None, None, None]).unwrap()
}

fn sample_track(id: u16, path: &str) -> TrackDefinition {
    TrackDefinition {
        id: TrackId(id),
        name: format!("Sample {id}"),
        kind: TrackKind::Sample,
        gain: 1.0,
        pan: 0.0,
        muted: false,
        soloed: false,
        pattern: kick_pattern(),
        synth_patch: None,
        sample: Some(SampleSettings::new(path, SampleMode::OneShot)),
    }
}

fn synth_track(id: u16) -> TrackDefinition {
    TrackDefinition {
        kind: TrackKind::Synth,
        sample: None,
        ..sample_track(id, "unused")
    }
}

fn project(tracks: Vec<TrackDefinition>) -> MultiTrackProject {
    MultiTrackProject {
        schema_version: 2,
        revision: 1,
        seed: 7,
        bpm: 120.0,
        steps_per_beat: 4,
        tracks,
    }
}

#[test]
fn sample_track_definitions_must_match_their_kind() {
    let mut missing = sample_track(1, "kick.wav");
    missing.sample = None;
    assert!(missing
        .validate()
        .unwrap_err()
        .contains("needs a \"sample\""));

    let mut on_synth = synth_track(2);
    on_synth.sample = Some(SampleSettings::new("kick.wav", SampleMode::OneShot));
    assert!(on_synth.validate().is_err());
}

#[test]
fn project_mixes_synth_and_sample_tracks_deterministically() {
    let dir = tempfile::tempdir().unwrap();
    write_wav(&dir.path().join("kick.wav"), 44_100, 1, 4_410, |frame| {
        (1.0 - frame as f32 / 4_410.0) * 0.8
    });
    let project = project(vec![synth_track(1), sample_track(2, "kick.wav")]);

    let saved = dir.path().join("song.json");
    project.save_atomic(&saved).unwrap();
    let reloaded = MultiTrackProject::load(&saved).unwrap();
    assert_eq!(reloaded, project);

    let render = || {
        let bank = reloaded
            .load_sample_assets(dir.path(), DEFAULT_SAMPLE_MEMORY_BUDGET)
            .unwrap();
        let mut engine = MultiTrackEngine::with_sample_assets(
            48_000,
            reloaded.bpm,
            u32::from(reloaded.steps_per_beat),
            reloaded.seed,
            4,
            reloaded.tracks.clone(),
            Some(SampleContext {
                assets: &bank,
                base_dir: dir.path(),
            }),
        )
        .unwrap();
        (0..24_000)
            .map(|_| engine.next_stereo_frame())
            .collect::<Vec<_>>()
    };
    let first = render();
    assert!(first.iter().any(|(left, _)| left.abs() > 0.1));
    assert!(first.iter().all(|(l, r)| l.is_finite() && r.is_finite()));
    assert_eq!(first, render(), "same project renders identically");

    // Sample-only rendering hits the kick at frame 0 and again one beat later.
    let bank = reloaded
        .load_sample_assets(dir.path(), DEFAULT_SAMPLE_MEMORY_BUDGET)
        .unwrap();
    let mut kick_only = MultiTrackEngine::with_sample_assets(
        48_000,
        120.0,
        4,
        7,
        4,
        vec![sample_track(2, "kick.wav")],
        Some(SampleContext {
            assets: &bank,
            base_dir: dir.path(),
        }),
    )
    .unwrap();
    let out: Vec<f32> = (0..24_100)
        .map(|_| kick_only.next_stereo_frame().0)
        .collect();
    assert!(out[0] > 0.5);
    assert_eq!(out[5_000], 0.0, "0.1 s kick has finished");
    assert!(out[24_000] > 0.5, "next beat retriggers the kick");
}

#[test]
fn missing_samples_are_all_reported_and_leave_the_active_bank_untouched() {
    let dir = tempfile::tempdir().unwrap();
    write_wav(&dir.path().join("kick.wav"), 48_000, 1, 10, |_| 0.5);
    let good = project(vec![sample_track(1, "kick.wav")]);
    let active = good
        .load_sample_assets(dir.path(), DEFAULT_SAMPLE_MEMORY_BUDGET)
        .unwrap();

    let broken = project(vec![
        sample_track(1, "kick.wav"),
        sample_track(2, "missing-snare.wav"),
        sample_track(3, "missing-hat.wav"),
    ]);
    let error = broken
        .load_sample_assets(dir.path(), DEFAULT_SAMPLE_MEMORY_BUDGET)
        .unwrap_err();
    assert!(error.contains("2 sample file(s)"), "{error}");
    assert!(error.contains("missing-snare.wav") && error.contains("missing-hat.wav"));

    assert_eq!(active.len(), 1);
    assert!(active.id_for_path(dir.path().join("kick.wav")).is_some());

    // An engine cannot be built for a sample the bank does not hold.
    let engine = MultiTrackEngine::with_sample_assets(
        48_000,
        120.0,
        4,
        1,
        4,
        broken.tracks.clone(),
        Some(SampleContext {
            assets: &active,
            base_dir: dir.path(),
        }),
    );
    assert!(engine.unwrap_err().contains("has not been loaded"));
    assert!(MultiTrackEngine::new(48_000, 120.0, 4, 1, 4, good.tracks.clone()).is_err());
}

#[test]
fn decode_file_names_the_path_in_errors() {
    let error = decode_wav_file("definitely/not/here.wav", usize::MAX).unwrap_err();
    assert!(error.contains("definitely/not/here.wav"), "{error}");
}

#[test]
fn shared_frames_are_not_copied_per_track() {
    let asset = constant_asset(100, 48_000);
    let before = Arc::strong_count(&asset.frames);
    let playback =
        CompiledSamplePlayback::new(&asset, &settings(SampleMode::OneShot), 48_000).unwrap();
    assert_eq!(Arc::strong_count(&asset.frames), before + 1);
    drop(playback);
}

#[test]
fn bundled_sample_project_loads_and_renders() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("projects/example-samples.json");
    let project = MultiTrackProject::load(&path).unwrap();
    let base_dir = path.parent().unwrap();
    let bank = project
        .load_sample_assets(base_dir, DEFAULT_SAMPLE_MEMORY_BUDGET)
        .unwrap();
    assert_eq!(bank.len(), 3);
    let mut engine = MultiTrackEngine::with_sample_assets(
        48_000,
        project.bpm,
        u32::from(project.steps_per_beat),
        project.seed,
        16,
        project.tracks.clone(),
        Some(SampleContext {
            assets: &bank,
            base_dir,
        }),
    )
    .unwrap();
    let mut peak = 0.0_f32;
    for _ in 0..(48_000 * 4) {
        let (left, right) = engine.next_stereo_frame();
        assert!(left.is_finite() && right.is_finite());
        peak = peak.max(left.abs()).max(right.abs());
    }
    assert!(peak > 0.3 && peak < 1.0, "peak {peak}");
}
