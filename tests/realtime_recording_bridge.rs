use shelloop::{spawn_realtime_recording, RealtimeRecordingBridge, WavRecordingConfig};
use tempfile::tempdir;

#[test]
fn realtime_bridge_rejects_zero_block_frames() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.wav");
    let config = WavRecordingConfig::new(48_000, 1, 4).unwrap();
    assert!(RealtimeRecordingBridge::spawn(&path, config, 0).is_err());
}

#[test]
fn realtime_bridge_writes_full_and_partial_blocks() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("live.wav");
    let config = WavRecordingConfig::new(48_000, 1, 4).unwrap();
    let mut bridge = RealtimeRecordingBridge::spawn(&path, config, 4).unwrap();

    for sample in [0.1, 0.2, 0.3, 0.4, 0.5, 0.6] {
        bridge.push_sample(sample);
    }

    let summary = bridge.finish().unwrap();
    assert_eq!(summary.samples_written, 6);
    assert_eq!(summary.frames_written, 6);
    assert_eq!(summary.dropped_blocks, 0);

    let mut reader = hound::WavReader::open(path).unwrap();
    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples, vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6]);
}

#[test]
fn realtime_bridge_accepts_interleaved_multichannel_frames() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("stereo.wav");
    let config = WavRecordingConfig::new(44_100, 2, 4).unwrap();
    let mut bridge = RealtimeRecordingBridge::spawn(&path, config, 2).unwrap();

    assert!(bridge.push_frame(&[0.25, -0.25]));
    assert!(bridge.push_frame(&[0.5, -0.5]));
    assert!(!bridge.push_frame(&[0.75]));

    let summary = bridge.finish().unwrap();
    assert_eq!(summary.frames_written, 2);
    assert_eq!(summary.samples_written, 4);
    assert_eq!(summary.rejected_blocks, 1);
}

#[test]
fn realtime_bridge_reports_preallocated_pool_size() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("pool.wav");
    let config = WavRecordingConfig::new(48_000, 1, 3).unwrap();
    let bridge = RealtimeRecordingBridge::spawn(&path, config, 64).unwrap();

    assert_eq!(bridge.block_samples(), 64);
    assert_eq!(bridge.preallocated_blocks(), 4);
    bridge.finish().unwrap();
}


#[test]
fn split_realtime_recording_finalizes_after_producer_drop() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("split.wav");
    let config = WavRecordingConfig::new(48_000, 1, 4).unwrap();
    let (mut producer, finalizer) =
        spawn_realtime_recording(&path, config, 4).expect("split recorder should start");

    for sample in [0.2, 0.4, 0.6, 0.8, 1.0] {
        assert!(producer.push_sample(sample));
    }

    drop(producer);
    let summary = finalizer.finish().unwrap();
    assert_eq!(summary.frames_written, 5);
    assert_eq!(summary.samples_written, 5);
    assert_eq!(summary.dropped_blocks, 0);

    let mut reader = hound::WavReader::open(path).unwrap();
    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples, vec![0.2, 0.4, 0.6, 0.8, 1.0]);
}
