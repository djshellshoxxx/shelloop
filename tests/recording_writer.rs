use shelloop::{RecordingWriter, WavRecordingConfig};
use tempfile::tempdir;

#[test]
fn wav_recording_config_rejects_invalid_values() {
    assert!(WavRecordingConfig::new(0, 2, 8).is_err());
    assert!(WavRecordingConfig::new(48_000, 0, 8).is_err());
    assert!(WavRecordingConfig::new(48_000, 2, 0).is_err());
    assert!(WavRecordingConfig::new(48_000, 65, 8).is_err());
}

#[test]
fn background_writer_persists_float_samples_and_finalizes_header() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("take.wav");
    let config = WavRecordingConfig::new(48_000, 2, 4).unwrap();
    let writer = RecordingWriter::spawn(&path, config).unwrap();

    assert!(writer.try_record(vec![0.25, -0.25, 0.5, -0.5]));
    let summary = writer.finish().unwrap();

    assert_eq!(summary.sample_rate, 48_000);
    assert_eq!(summary.channels, 2);
    assert_eq!(summary.frames_written, 2);
    assert_eq!(summary.samples_written, 4);
    assert_eq!(summary.dropped_blocks, 0);

    let mut reader = hound::WavReader::open(path).unwrap();
    let spec = reader.spec();
    assert_eq!(spec.sample_rate, 48_000);
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.sample_format, hound::SampleFormat::Float);
    assert_eq!(spec.bits_per_sample, 32);

    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples, vec![0.25, -0.25, 0.5, -0.5]);
}

#[test]
fn writer_rejects_blocks_that_do_not_contain_complete_frames() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("bad.wav");
    let config = WavRecordingConfig::new(44_100, 2, 2).unwrap();
    let writer = RecordingWriter::spawn(&path, config).unwrap();

    assert!(!writer.try_record(vec![0.1, 0.2, 0.3]));
    let summary = writer.finish().unwrap();
    assert_eq!(summary.frames_written, 0);
    assert_eq!(summary.samples_written, 0);
    assert_eq!(summary.rejected_blocks, 1);
}

#[test]
fn writer_sanitizes_non_finite_and_out_of_range_samples() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("sanitize.wav");
    let config = WavRecordingConfig::new(48_000, 1, 2).unwrap();
    let writer = RecordingWriter::spawn(&path, config).unwrap();

    assert!(writer.try_record(vec![f32::NAN, f32::INFINITY, 2.0, -2.0]));
    writer.finish().unwrap();

    let mut reader = hound::WavReader::open(path).unwrap();
    let samples: Vec<f32> = reader.samples::<f32>().map(Result::unwrap).collect();
    assert_eq!(samples, vec![0.0, 0.0, 1.0, -1.0]);
}
