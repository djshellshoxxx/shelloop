//! Spec 11 — retrospective performance black box (no audio device required).

use sha2::{Digest, Sha256};
use shelloop::blackbox::{spawn_black_box_tuned, BlackBoxGate, BlackBoxTuning};
use shelloop::{
    quick_save_file_stem, spawn_black_box, BlackBoxConfig, BlackBoxHandle, BlackBoxProducer,
    BlackBoxStatus, ControlEvent, ControlEventKind, EventSource, SaveOutcome, SaveTarget,
    SavedTake, SessionMetadata, BLACK_BOX_SIDECAR_SCHEMA, DEFAULT_BLACK_BOX_MEMORY_BUDGET,
    MAX_BLACK_BOX_SECONDS, MIN_BLACK_BOX_SECONDS,
};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(10);

fn metadata() -> SessionMetadata {
    SessionMetadata {
        session_id: "abc123def456".into(),
        engine_version: "test-engine".into(),
        content_hash: Some("deadbeef".into()),
        bpm: Some(120.0),
        steps_per_beat: Some(4),
        seed: Some(42),
    }
}

/// A tiny sample rate keeps the rolling window short so wraparound is cheap to test.
fn small_config(dir: &Path, seconds: u32, sample_rate: u32) -> BlackBoxConfig {
    BlackBoxConfig::new(
        seconds,
        sample_rate,
        dir.to_path_buf(),
        DEFAULT_BLACK_BOX_MEMORY_BUDGET,
    )
    .expect("valid config")
}

fn tuning(block_frames: usize, pool_blocks: usize, start_paused: bool) -> BlackBoxTuning {
    BlackBoxTuning {
        block_frames,
        pool_blocks,
        start_paused,
    }
}

fn spawn_tuned(
    config: BlackBoxConfig,
    tuning: BlackBoxTuning,
) -> (BlackBoxProducer, BlackBoxHandle, BlackBoxGate) {
    spawn_black_box_tuned(config, metadata(), tuning).expect("spawn black box")
}

fn sample_for(frame: u64, channel: u64) -> f32 {
    // Distinct, exactly representable values per frame/channel.
    ((frame * 2 + channel) % 1_000_003) as f32 * 0.5 - 3.0
}

fn push_range(producer: &mut BlackBoxProducer, start: u64, end: u64) {
    for frame in start..end {
        producer.push_frame(frame, sample_for(frame, 0), sample_for(frame, 1));
    }
}

fn wait_status(
    handle: &BlackBoxHandle,
    mut predicate: impl FnMut(&BlackBoxStatus) -> bool,
) -> BlackBoxStatus {
    let deadline = Instant::now() + WAIT;
    loop {
        let status = handle.status();
        if predicate(&status) {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for status: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn wait_newest(handle: &BlackBoxHandle, newest: u64) -> BlackBoxStatus {
    wait_status(handle, |status| status.newest_frame == Some(newest))
}

fn wait_outcome(handle: &BlackBoxHandle) -> SaveOutcome {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(outcome) = handle.try_outcome() {
            return outcome;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for save outcome"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn save(handle: &BlackBoxHandle, target: SaveTarget, endpoint: u64) -> Result<SavedTake, String> {
    let id = handle.request_save(target, endpoint).expect("queue save");
    let outcome = wait_outcome(handle);
    assert_eq!(outcome.request_id, id);
    outcome.result
}

fn read_wav(path: &Path) -> (hound::WavSpec, Vec<f32>) {
    let mut reader = hound::WavReader::open(path).expect("open wav");
    let spec = reader.spec();
    let samples = reader
        .samples::<f32>()
        .map(|sample| sample.expect("sample"))
        .collect();
    (spec, samples)
}

fn read_sidecar(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).expect("read sidecar")).expect("parse sidecar")
}

fn expected_samples(start: u64, end: u64) -> Vec<f32> {
    (start..end)
        .flat_map(|frame| [sample_for(frame, 0), sample_for(frame, 1)])
        .collect()
}

fn assert_bit_exact(actual: &[f32], expected: &[f32]) {
    assert_eq!(actual.len(), expected.len());
    for (index, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert_eq!(a.to_bits(), e.to_bits(), "sample {index} differs");
    }
}

fn dir_entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|entry| entry.expect("entry").file_name().to_string_lossy().into())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

// ---------------------------------------------------------------- config

#[test]
fn config_rejects_invalid_bounds() {
    let dir = PathBuf::from("takes");
    let budget = DEFAULT_BLACK_BOX_MEMORY_BUDGET;
    assert!(BlackBoxConfig::new(0, 48_000, dir.clone(), budget).is_err());
    assert!(BlackBoxConfig::new(MAX_BLACK_BOX_SECONDS + 1, 48_000, dir.clone(), budget).is_err());
    assert!(BlackBoxConfig::new(10, 0, dir.clone(), budget).is_err());
    assert!(BlackBoxConfig::new(10, 48_000, PathBuf::new(), budget).is_err());
    let min = BlackBoxConfig::new(MIN_BLACK_BOX_SECONDS, 48_000, dir.clone(), budget).unwrap();
    assert_eq!(min.effective_frames, 48_000);
    assert_eq!(min.channels, 2);
    let max = BlackBoxConfig::new(MAX_BLACK_BOX_SECONDS, 44_100, dir, budget).unwrap();
    assert_eq!(max.requested_seconds, MAX_BLACK_BOX_SECONDS);
    assert_eq!(max.effective_frames, 120 * 44_100);
    assert!((max.effective_seconds() - 120.0).abs() < 1e-9);
}

#[test]
fn config_clamps_to_memory_budget_and_reports_bytes_when_too_small() {
    let dir = PathBuf::from("takes");
    // 120 s at 192 kHz stereo f32 = ~176 MiB, more than the default 128 MiB budget.
    let clamped =
        BlackBoxConfig::new(120, 192_000, dir.clone(), DEFAULT_BLACK_BOX_MEMORY_BUDGET).unwrap();
    assert!(clamped.effective_frames < 120 * 192_000);
    assert!(clamped.effective_frames * 8 <= DEFAULT_BLACK_BOX_MEMORY_BUDGET as u64);
    assert!(clamped.effective_seconds() > 80.0 && clamped.effective_seconds() < 120.0);

    let error = BlackBoxConfig::new(10, 48_000, dir, 100_000).unwrap_err();
    assert!(error.contains("100000"), "{error}");
    assert!(error.contains("bytes"), "{error}");
    assert!(
        error.contains("384000") || error.contains("required"),
        "{error}"
    );
}

#[test]
fn config_does_not_create_directory() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("shelloop-takes");
    let _config = small_config(&dir, 2, 1_000);
    assert!(!dir.exists());
}

#[test]
fn quick_save_stem_uses_utc_civil_time() {
    assert_eq!(
        quick_save_file_stem(1_791_383_130, "abc123", 1),
        "shelloop-take-20261007-142530Z-abc123-01"
    );
    assert_eq!(
        quick_save_file_stem(0, "s", 12),
        "shelloop-take-19700101-000000Z-s-12"
    );
    assert_eq!(
        quick_save_file_stem(1_709_251_199, "x", 3),
        "shelloop-take-20240229-235959Z-x-03"
    );
}

// ---------------------------------------------------------------- events

#[test]
fn control_event_validation_rejects_malformed_fields() {
    let event = |kind| ControlEvent {
        sequence: 0,
        frame: 0,
        source: EventSource::Midi,
        kind,
    };
    assert!(event(ControlEventKind::NoteOn {
        channel: 0,
        note: 60,
        velocity: 0.8
    })
    .validate()
    .is_ok());
    assert!(event(ControlEventKind::NoteOn {
        channel: 16,
        note: 60,
        velocity: 0.8
    })
    .validate()
    .is_err());
    assert!(event(ControlEventKind::NoteOff {
        channel: 0,
        note: 128
    })
    .validate()
    .is_err());
    assert!(event(ControlEventKind::NoteOn {
        channel: 0,
        note: 1,
        velocity: f32::NAN
    })
    .validate()
    .is_err());
    assert!(event(ControlEventKind::XyMix {
        live_gain: f32::INFINITY,
        sequencer_gain: 0.0
    })
    .validate()
    .is_err());
    assert!(event(ControlEventKind::TempoChange { bpm: f64::NAN })
        .validate()
        .is_err());
    assert!(event(ControlEventKind::EditorCommand {
        command: "x".repeat(257)
    })
    .validate()
    .is_err());
    assert!(event(ControlEventKind::EditorCommand {
        command: "x".repeat(256)
    })
    .validate()
    .is_ok());
}

#[test]
fn control_event_json_round_trip() {
    let event = ControlEvent {
        sequence: 7,
        frame: 99,
        source: EventSource::Keyboard,
        kind: ControlEventKind::RevisionQueued {
            track: 2,
            revision: 5,
            apply_at_frame: 1024,
        },
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "revision_queued");
    assert_eq!(json["source"], "keyboard");
    let back: ControlEvent = serde_json::from_value(json).unwrap();
    assert_eq!(back, event);
}

// ---------------------------------------------------------------- ring

#[test]
fn ring_wraparound_keeps_exactly_newest_frames_interleaved() {
    let temp = tempfile::tempdir().unwrap();
    let config = small_config(temp.path(), 1, 1_000); // 1000 frame window
    let (mut producer, handle, _gate) = spawn_tuned(config, tuning(64, 64, false));

    // Push 3.5 windows in arbitrary-ish chunks; frames start at an offset.
    let start = 10_000_u64;
    let end = start + 3_500;
    push_range(&mut producer, start, end);
    producer.flush();
    let status = wait_newest(&handle, end - 1);
    assert_eq!(status.oldest_frame, Some(end - 1_000));

    let take = save(
        &handle,
        SaveTarget::Path(temp.path().join("wrap.wav")),
        u64::MAX,
    )
    .unwrap();
    assert_eq!(take.start_frame, end - 1_000);
    assert_eq!(take.end_frame, end);
    assert!(take.complete);
    assert!(take.missing_ranges.is_empty());

    let (spec, samples) = read_wav(&take.wav_path);
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.sample_rate, 1_000);
    assert_eq!(spec.bits_per_sample, 32);
    assert_eq!(spec.sample_format, hound::SampleFormat::Float);
    assert_bit_exact(&samples, &expected_samples(end - 1_000, end));
    handle.shutdown(WAIT).unwrap();
}

#[test]
fn wav_preserves_silence_clipping_and_special_values_bit_exactly() {
    let temp = tempfile::tempdir().unwrap();
    let config = small_config(temp.path(), 1, 1_000);
    let (mut producer, handle, _gate) = spawn_tuned(config, tuning(16, 16, false));
    let values = [
        0.0_f32,
        -0.0,
        1.0,
        -1.0,
        3.75,
        -12.5,
        f32::MIN_POSITIVE / 4.0,
        1.0e-30,
    ];
    let mut expected = Vec::new();
    for frame in 0..64_u64 {
        let left = values[frame as usize % values.len()];
        let right = values[(frame as usize + 3) % values.len()];
        producer.push_frame(frame, left, right);
        expected.extend([left, right]);
    }
    producer.flush();
    wait_newest(&handle, 63);
    let take = save(&handle, SaveTarget::Path(temp.path().join("exact")), 64).unwrap();
    assert_eq!(take.wav_path, temp.path().join("exact.wav"));
    assert_eq!(take.sidecar_path, temp.path().join("exact.json"));
    let (_, samples) = read_wav(&take.wav_path);
    assert_bit_exact(&samples, &expected);
    handle.shutdown(WAIT).unwrap();
}

#[test]
fn save_interval_short_session_and_endpoint_clamping() {
    let temp = tempfile::tempdir().unwrap();
    let config = small_config(temp.path(), 2, 1_000); // 2000 frame window
    let (mut producer, handle, _gate) = spawn_tuned(config, tuning(32, 32, false));

    // No audio yet: save fails but the black box keeps running.
    assert!(save(&handle, SaveTarget::Path(temp.path().join("none.wav")), 10).is_err());

    // Short session: 500 frames starting at frame 100.
    push_range(&mut producer, 100, 600);
    producer.flush();
    wait_newest(&handle, 599);

    // Endpoint beyond newest clamps to the newest frame.
    let take = save(&handle, SaveTarget::Path(temp.path().join("a.wav")), 9_999).unwrap();
    assert_eq!((take.start_frame, take.end_frame), (100, 600));
    let (_, samples) = read_wav(&take.wav_path);
    assert_bit_exact(&samples, &expected_samples(100, 600));

    // Endpoint in the middle ends the take there.
    let take = save(&handle, SaveTarget::Path(temp.path().join("b.wav")), 350).unwrap();
    assert_eq!((take.start_frame, take.end_frame), (100, 350));
    let (_, samples) = read_wav(&take.wav_path);
    assert_bit_exact(&samples, &expected_samples(100, 350));
    let sidecar = read_sidecar(&take.sidecar_path);
    assert_eq!(sidecar["save_request_frame"], 350);
    assert_eq!(sidecar["start_frame"], 100);
    assert_eq!(sidecar["end_frame"], 350);

    // Endpoint before retained history is an error.
    assert!(save(&handle, SaveTarget::Path(temp.path().join("c.wav")), 50).is_err());
    assert!(!temp.path().join("c.wav").exists());
    handle.shutdown(WAIT).unwrap();
}

#[test]
fn arbitrary_push_chunk_sizes_give_correct_frame_totals() {
    let temp = tempfile::tempdir().unwrap();
    let config = small_config(temp.path(), 5, 1_000); // 5000 frame window
    let (mut producer, handle, _gate) = spawn_tuned(config, tuning(48, 128, false));
    let chunks = [1_usize, 7, 48, 47, 49, 96, 333, 2, 512, 1000, 13, 255];
    let mut frame = 0_u64;
    let mut interleaved = Vec::new();
    for (index, chunk) in chunks.iter().cycle().take(40).enumerate() {
        interleaved.clear();
        for offset in 0..*chunk as u64 {
            interleaved.push(sample_for(frame + offset, 0));
            interleaved.push(sample_for(frame + offset, 1));
        }
        if index % 2 == 0 {
            producer.push_interleaved(frame, &interleaved);
        } else {
            for (offset, [left, right]) in interleaved.as_chunks::<2>().0.iter().enumerate() {
                producer.push_frame(frame + offset as u64, *left, *right);
            }
        }
        frame += *chunk as u64;
        // Avoid overrunning the pool in this test: let the collector keep up.
        if index % 8 == 7 {
            producer.flush();
            wait_newest(&handle, frame - 1);
        }
    }
    producer.flush();
    let status = wait_newest(&handle, frame - 1);
    assert_eq!(status.dropped_audio_frames, 0);
    let window = 5_000.min(frame);
    assert_eq!(status.oldest_frame, Some(frame - window));
    let take = save(
        &handle,
        SaveTarget::Path(temp.path().join("chunks.wav")),
        frame,
    )
    .unwrap();
    assert!(take.complete);
    assert_eq!(take.end_frame - take.start_frame, window);
    let (_, samples) = read_wav(&take.wav_path);
    assert_bit_exact(&samples, &expected_samples(frame - window, frame));
    handle.shutdown(WAIT).unwrap();
}

// ---------------------------------------------------------------- drops

#[test]
fn stalled_collector_never_blocks_producer_and_exposes_missing_ranges() {
    let temp = tempfile::tempdir().unwrap();
    let config = small_config(temp.path(), 10, 1_000); // 10000 frame window
    let block = 64_u64;
    let pool = 4_usize;
    let (mut producer, handle, gate) = spawn_tuned(config, tuning(block as usize, pool, true));

    // While the collector is stalled, push far more than the pool can hold.
    let started = Instant::now();
    push_range(&mut producer, 0, 2_000);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "producer must never block"
    );
    let accepted = block * pool as u64;
    assert_eq!(producer.dropped_frames(), 2_000 - accepted);

    gate.release();
    wait_newest(&handle, accepted - 1);
    // Collector has returned blocks to the pool; capture resumes after a gap.
    push_range(&mut producer, 2_000, 2_000 + block * 2);
    producer.flush();
    let status = wait_newest(&handle, 2_000 + block * 2 - 1);
    assert_eq!(status.dropped_audio_frames, 2_000 - accepted);

    let take = save(
        &handle,
        SaveTarget::Path(temp.path().join("gap.wav")),
        u64::MAX,
    )
    .unwrap();
    assert_eq!(take.start_frame, 0);
    assert_eq!(take.end_frame, 2_000 + block * 2);
    assert!(!take.complete);
    assert_eq!(take.missing_ranges, vec![(accepted, 2_000)]);

    let sidecar = read_sidecar(&take.sidecar_path);
    assert_eq!(sidecar["complete"], false);
    assert_eq!(
        sidecar["missing_audio_ranges"],
        serde_json::json!([[accepted, 2_000]])
    );
    assert_eq!(sidecar["dropped_audio_frames"], 2_000 - accepted);

    // Captured frames are exact; the gap is not presented as real audio data
    // (it is declared missing in the sidecar and holds silence in the WAV).
    let (_, samples) = read_wav(&take.wav_path);
    assert_eq!(samples.len() as u64, (2_000 + block * 2) * 2);
    assert_bit_exact(
        &samples[..(accepted * 2) as usize],
        &expected_samples(0, accepted),
    );
    assert!(samples[(accepted * 2) as usize..4_000]
        .iter()
        .all(|sample| *sample == 0.0));
    assert_bit_exact(
        &samples[4_000..],
        &expected_samples(2_000, 2_000 + block * 2),
    );
    handle.shutdown(WAIT).unwrap();
}

#[test]
fn full_queue_shutdown_does_not_deadlock() {
    let temp = tempfile::tempdir().unwrap();
    let config = small_config(temp.path(), 2, 1_000);
    let (mut producer, handle, gate) = spawn_tuned(config, tuning(16, 2, true));
    push_range(&mut producer, 0, 500);
    for frame in 0..10_000 {
        handle.record_event(frame, EventSource::Cli, ControlEventKind::Panic);
    }
    while handle
        .request_save(SaveTarget::Path(temp.path().join("never.wav")), 10)
        .is_ok()
    {}
    let started = Instant::now();
    let result = handle.shutdown(Duration::from_secs(2));
    assert!(result.is_ok(), "{result:?}");
    assert!(started.elapsed() < Duration::from_secs(3));
    // Producer keeps working (dropping) after shutdown.
    push_range(&mut producer, 500, 1_000);
    assert!(producer.dropped_frames() >= 500);
    drop(gate);
}

#[test]
fn save_queue_is_bounded() {
    let temp = tempfile::tempdir().unwrap();
    let config = small_config(temp.path(), 2, 1_000);
    let (_producer, handle, gate) = spawn_tuned(config, tuning(16, 4, true));
    let mut accepted = 0;
    let mut ids = Vec::new();
    for index in 0..20 {
        match handle.request_save(
            SaveTarget::Path(temp.path().join(format!("q{index}.wav"))),
            10,
        ) {
            Ok(id) => {
                accepted += 1;
                ids.push(id);
            }
            Err(error) => assert!(!error.is_empty()),
        }
    }
    assert!((1..20).contains(&accepted), "accepted {accepted}");
    assert_eq!(handle.status().pending_saves, accepted);
    let mut sorted = ids.clone();
    sorted.dedup();
    assert_eq!(sorted.len(), ids.len());
    gate.release();
    // All fail (no audio captured) and report in order.
    for id in ids {
        let outcome = wait_outcome(&handle);
        assert_eq!(outcome.request_id, id);
        assert!(outcome.result.is_err());
    }
    wait_status(&handle, |status| status.pending_saves == 0);
    handle.shutdown(WAIT).unwrap();
}

// ---------------------------------------------------------------- sidecar

#[test]
fn sidecar_events_sorted_by_frame_then_sequence_and_hash_matches() {
    let temp = tempfile::tempdir().unwrap();
    let config = small_config(temp.path(), 2, 1_000);
    let (mut producer, handle, _gate) = spawn_tuned(config, tuning(32, 64, false));
    push_range(&mut producer, 0, 1_000);
    producer.flush();
    wait_newest(&handle, 999);

    // Recorded out of frame order; equal frames must keep sequence order.
    assert!(handle.record_event(
        500,
        EventSource::Midi,
        ControlEventKind::NoteOn {
            channel: 1,
            note: 64,
            velocity: 0.5
        }
    ));
    assert!(handle.record_event(
        200,
        EventSource::Keyboard,
        ControlEventKind::NoteOff {
            channel: 0,
            note: 60
        }
    ));
    assert!(handle.record_event(
        500,
        EventSource::Mouse,
        ControlEventKind::XyMix {
            live_gain: 0.25,
            sequencer_gain: 0.75
        }
    ));
    assert!(handle.record_event(500, EventSource::Cli, ControlEventKind::Panic));
    // Outside the saved interval (endpoint 900).
    assert!(handle.record_event(
        950,
        EventSource::Internal,
        ControlEventKind::TempoChange { bpm: 128.0 }
    ));
    // Malformed: rejected and counted, never recorded.
    assert!(!handle.record_event(
        500,
        EventSource::Midi,
        ControlEventKind::NoteOn {
            channel: 99,
            note: 1,
            velocity: 1.0
        }
    ));
    wait_status(&handle, |status| status.rejected_events == 1);

    let take = save(&handle, SaveTarget::Path(temp.path().join("ev.wav")), 900).unwrap();
    assert!(take.complete);
    let sidecar = read_sidecar(&take.sidecar_path);
    assert_eq!(sidecar["schema"], BLACK_BOX_SIDECAR_SCHEMA);
    assert_eq!(sidecar["engine_version"], "test-engine");
    assert_eq!(sidecar["session_id"], "abc123def456");
    assert_eq!(sidecar["sample_rate"], 1_000);
    assert_eq!(sidecar["channels"], 2);
    assert_eq!(sidecar["requested_seconds"], 2);
    assert_eq!(sidecar["effective_seconds"], 2.0);
    assert_eq!(sidecar["content_hash"], "deadbeef");
    assert_eq!(sidecar["bpm"], 120.0);
    assert_eq!(sidecar["steps_per_beat"], 4);
    assert_eq!(sidecar["seed"], 42);
    assert_eq!(sidecar["complete"], true);
    assert_eq!(sidecar["rejected_events"], 1);

    let events = sidecar["events"].as_array().unwrap();
    let keys: Vec<(u64, u64)> = events
        .iter()
        .map(|event| {
            (
                event["frame"].as_u64().unwrap(),
                event["sequence"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(keys.len(), 4);
    assert_eq!(keys[0].0, 200);
    assert!(keys[1..].iter().all(|(frame, _)| *frame == 500));
    assert!(keys.windows(2).all(|pair| pair[0] < pair[1]));
    assert_eq!(events[1]["kind"], "note_on");
    assert_eq!(events[2]["kind"], "xy_mix");
    assert_eq!(events[3]["kind"], "panic");
    assert_eq!(events[3]["source"], "cli");

    let digest = Sha256::digest(std::fs::read(&take.wav_path).unwrap());
    let hex: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(sidecar["wav_sha256"], hex);
    handle.shutdown(WAIT).unwrap();
}

// ---------------------------------------------------------------- persistence

#[test]
fn explicit_existing_path_is_protected() {
    let temp = tempfile::tempdir().unwrap();
    let config = small_config(temp.path(), 1, 1_000);
    let (mut producer, handle, _gate) = spawn_tuned(config, tuning(32, 64, false));
    push_range(&mut producer, 0, 256);
    producer.flush();
    wait_newest(&handle, 255);

    let wav = temp.path().join("keep.wav");
    std::fs::write(&wav, b"precious").unwrap();
    let error = save(&handle, SaveTarget::Path(wav.clone()), 256).unwrap_err();
    assert!(error.contains("exist"), "{error}");
    assert_eq!(std::fs::read(&wav).unwrap(), b"precious");
    assert!(!temp.path().join("keep.json").exists());

    // An existing sidecar also protects the pair.
    let json = temp.path().join("other.json");
    std::fs::write(&json, b"{}").unwrap();
    assert!(save(
        &handle,
        SaveTarget::Path(temp.path().join("other.wav")),
        256
    )
    .is_err());
    assert_eq!(std::fs::read(&json).unwrap(), b"{}");
    assert!(!temp.path().join("other.wav").exists());
    assert_eq!(dir_entries(temp.path()), vec!["keep.wav", "other.json"]);
    handle.shutdown(WAIT).unwrap();
}

#[test]
fn quick_save_creates_directory_lazily_and_never_collides() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("nested").join("takes");
    let config = small_config(&dir, 1, 1_000);
    let (mut producer, handle, _gate) = spawn_tuned(config, tuning(32, 64, false));
    push_range(&mut producer, 0, 512);
    producer.flush();
    wait_newest(&handle, 511);
    assert!(!dir.exists());

    let first = save(&handle, SaveTarget::QuickSave, 512).unwrap();
    let second = save(&handle, SaveTarget::QuickSave, 512).unwrap();
    assert!(dir.is_dir());
    assert_ne!(first.wav_path, second.wav_path);
    for take in [&first, &second] {
        assert_eq!(take.wav_path.parent().unwrap(), dir);
        let name = take.wav_path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("shelloop-take-"), "{name}");
        assert!(name.contains("abc123de"), "{name}");
        assert!(name.ends_with(".wav"));
        assert_eq!(take.sidecar_path, take.wav_path.with_extension("json"));
        assert!(take.sidecar_path.exists());
    }
    assert_eq!(dir_entries(&dir).len(), 4);
    assert!(dir_entries(&dir).iter().all(|name| !name.ends_with(".tmp")));
    handle.shutdown(WAIT).unwrap();
}

#[test]
fn write_failure_cleans_temporaries_and_history_remains_for_retry() {
    let temp = tempfile::tempdir().unwrap();
    // Quick-save directory is actually a file: creation fails.
    let blocker = temp.path().join("not-a-dir");
    std::fs::write(&blocker, b"file").unwrap();
    let config = small_config(&blocker, 1, 1_000);
    let (mut producer, handle, _gate) = spawn_tuned(config, tuning(32, 64, false));
    push_range(&mut producer, 0, 640);
    producer.flush();
    wait_newest(&handle, 639);

    assert!(save(&handle, SaveTarget::QuickSave, 640).is_err());
    assert_eq!(std::fs::read(&blocker).unwrap(), b"file");

    // Sidecar temporary path is occupied by a directory: the WAV temp is
    // written first and must be cleaned up when the sidecar write fails.
    let out = temp.path().join("out");
    std::fs::create_dir(&out).unwrap();
    std::fs::create_dir(out.join("fail.json.tmp")).unwrap();
    let error = save(&handle, SaveTarget::Path(out.join("fail.wav")), 640).unwrap_err();
    assert!(!error.is_empty());
    assert_eq!(dir_entries(&out), vec!["fail.json.tmp"]);

    // History is retained: a retry to a good path succeeds with the same audio.
    let take = save(&handle, SaveTarget::Path(out.join("retry.wav")), 640).unwrap();
    assert_eq!((take.start_frame, take.end_frame), (0, 640));
    let (_, samples) = read_wav(&take.wav_path);
    assert_bit_exact(&samples, &expected_samples(0, 640));
    assert!(handle.status().failed.is_none());
    handle.shutdown(WAIT).unwrap();
}

#[test]
fn clear_empties_history_without_deleting_files() {
    let temp = tempfile::tempdir().unwrap();
    let config = small_config(temp.path(), 1, 1_000);
    let (mut producer, handle, _gate) = spawn_tuned(config, tuning(32, 64, false));
    push_range(&mut producer, 0, 320);
    producer.flush();
    wait_newest(&handle, 319);
    let take = save(&handle, SaveTarget::Path(temp.path().join("kept.wav")), 320).unwrap();

    handle.clear().unwrap();
    wait_status(&handle, |status| status.newest_frame.is_none());
    assert!(take.wav_path.exists() && take.sidecar_path.exists());
    assert!(save(
        &handle,
        SaveTarget::Path(temp.path().join("empty.wav")),
        320
    )
    .is_err());

    // Capture resumes after the clear with only new frames.
    push_range(&mut producer, 320, 384);
    producer.flush();
    let status = wait_newest(&handle, 383);
    assert_eq!(status.oldest_frame, Some(320));
    let after = save(
        &handle,
        SaveTarget::Path(temp.path().join("after.wav")),
        384,
    )
    .unwrap();
    assert_eq!((after.start_frame, after.end_frame), (320, 384));
    assert!(after.complete);
    handle.shutdown(WAIT).unwrap();
}

#[test]
fn default_spawn_reports_armed_status() {
    let temp = tempfile::tempdir().unwrap();
    let config = BlackBoxConfig::new(
        3,
        48_000,
        temp.path().join("takes"),
        DEFAULT_BLACK_BOX_MEMORY_BUDGET,
    )
    .unwrap();
    let (mut producer, handle) = spawn_black_box(config, metadata()).unwrap();
    let status = handle.status();
    assert!(status.armed);
    assert!((status.effective_seconds - 3.0).abs() < 1e-9);
    assert_eq!(status.oldest_frame, None);
    assert!(status.failed.is_none());
    push_range(&mut producer, 0, 4_096);
    producer.flush();
    wait_newest(&handle, 4_095);
    drop(producer);
    handle.shutdown(WAIT).unwrap();
    assert!(!temp.path().join("takes").exists());
}
