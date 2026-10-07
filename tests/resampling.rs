use shelloop::{
    capture_channel, normalize_wav_file, parse_resample_command, resolve_capture_frames,
    CaptureClock, CompiledSamplePlayback, MultiTrackEngine, NormalizeOutcome, Pattern,
    PreparedTrack, QuantizeBoundary, ResampleCommand, ResampleDestination, ResampleRequest,
    ResampleSource, ResampleState, ResampleStop, Resampler, SampleAssetBank, SampleMode,
    SampleSettings, TrackDefinition, TrackId, TrackKind, MAX_REALTIME_TRACKS,
};
use std::time::{Duration, Instant};

const RATE: u32 = 48_000;
const CLOCK: CaptureClock = CaptureClock {
    sample_rate: RATE,
    bpm: 120.0,
    steps_per_beat: 4,
    beats_per_bar: 4,
};

fn request(stop: ResampleStop, normalize: bool) -> ResampleRequest {
    ResampleRequest {
        source: ResampleSource::Master,
        start_boundary: QuantizeBoundary::Beat,
        stop,
        normalize,
        destination: ResampleDestination::AssetOnly,
    }
}

/// Drive a tap through `frames` frames whose master value encodes the frame
/// index, polling the resampler between simulated callback blocks.
fn run_capture(
    resampler: &mut Resampler,
    tap: &mut shelloop::CaptureTap,
    frames: u64,
    block: u64,
) -> Option<shelloop::FinishedCapture> {
    let mut frame = 0;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        tap.poll_control();
        let end = (frame + block).min(frames);
        while frame < end {
            let value = (frame % 1000) as f32 / 1000.0;
            tap.process_frame(frame, (value, -value), Some((0.25, 0.25)));
            frame += 1;
        }
        if let Some(done) = resampler.poll(frame) {
            return Some(done);
        }
        if frame >= frames && Instant::now() > deadline {
            return None;
        }
        if frame >= frames {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn fixed_bars_capture_is_frame_exact_inside_blocks() {
    let dir = tempfile::tempdir().unwrap();
    let (control, mut tap, returns) = capture_channel();
    let mut resampler = Resampler::new(dir.path().to_path_buf(), RATE, control, returns);
    let (start, stop) = resampler
        .arm(request(ResampleStop::Bars(1), false), 100, CLOCK)
        .unwrap();
    assert_eq!(start, 24_000, "next beat after frame 100");
    assert_eq!(stop, Some(24_000 + 96_000));
    assert!(matches!(resampler.state(), ResampleState::Armed { .. }));
    assert!(std::fs::read_dir(dir.path()).unwrap().all(|entry| !entry
        .unwrap()
        .path()
        .to_string_lossy()
        .ends_with("1.wav")));

    let done = run_capture(&mut resampler, &mut tap, 130_000, 333).expect("finalized");
    assert_eq!(done.summary.frames_written, 96_000);
    assert!(!done.degraded);
    assert_eq!(done.decoded.frames.len(), 96_000);
    // First captured frame is exactly the start frame (24_000 % 1000 == 0).
    assert_eq!(done.decoded.frames[0].left, 0.0);
    assert_eq!(done.decoded.frames[1].left, 0.001);
    assert!(done.path.ends_with("Resample-0001.wav"));
    assert!(matches!(resampler.state(), ResampleState::Ready { .. }));

    // The resampled asset loads and plays through the sample engine.
    let mut bank = SampleAssetBank::new(usize::MAX);
    let id = bank.load_file(&done.path).unwrap();
    let settings = SampleSettings::new(done.path.to_string_lossy(), SampleMode::OneShot);
    CompiledSamplePlayback::new(bank.get(id).unwrap(), &settings, RATE).unwrap();
}

#[test]
fn manual_stop_excludes_frames_after_stop_and_names_increment() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Resample-0001.wav"), b"existing").unwrap();
    let (control, mut tap, returns) = capture_channel();
    let mut resampler = Resampler::new(dir.path().to_path_buf(), RATE, control, returns);
    let mut req = request(
        ResampleStop::Manual {
            boundary: QuantizeBoundary::Step,
        },
        false,
    );
    req.start_boundary = QuantizeBoundary::Immediate;
    resampler.arm(req, 0, CLOCK).unwrap();
    let stop = resampler.stop(10, CLOCK, QuantizeBoundary::Step).unwrap();
    assert_eq!(stop, 6_000);
    let done = run_capture(&mut resampler, &mut tap, 20_000, 512).unwrap();
    assert_eq!(done.summary.frames_written, 6_000);
    assert!(done.path.ends_with("Resample-0002.wav"));
}

#[test]
fn track_source_captures_track_output_and_cancel_leaves_no_files() {
    let dir = tempfile::tempdir().unwrap();
    let (control, mut tap, returns) = capture_channel();
    let mut resampler = Resampler::new(dir.path().to_path_buf(), RATE, control, returns);
    let mut req = request(ResampleStop::Seconds(0.1), false);
    req.source = ResampleSource::Track(TrackId(3));
    req.start_boundary = QuantizeBoundary::Immediate;
    resampler.arm(req, 0, CLOCK).unwrap();
    let done = run_capture(&mut resampler, &mut tap, 10_000, 64).unwrap();
    assert!(done.decoded.frames.iter().all(|frame| frame.left == 0.25));

    resampler.acknowledge();
    resampler
        .arm(request(ResampleStop::Bars(1), false), 0, CLOCK)
        .unwrap();
    resampler.cancel().unwrap();
    tap.poll_control();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !matches!(resampler.state(), ResampleState::Idle) && Instant::now() < deadline {
        resampler.poll(0);
    }
    assert!(matches!(resampler.state(), ResampleState::Idle));
    let names: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, vec!["Resample-0001.wav".to_string()]);
}

#[test]
fn normalization_hits_target_and_reports_silence() {
    let dir = tempfile::tempdir().unwrap();
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let loud = dir.path().join("loud.wav");
    let mut writer = hound::WavWriter::create(&loud, spec).unwrap();
    for value in [0.1_f32, -0.25, 0.2, 0.0] {
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();
    let outcome = normalize_wav_file(&loud, 0.891_250_9).unwrap();
    assert!(matches!(outcome, NormalizeOutcome::Normalized { .. }));
    let samples: Vec<f32> = hound::WavReader::open(&loud)
        .unwrap()
        .samples::<f32>()
        .map(Result::unwrap)
        .collect();
    let peak = samples
        .iter()
        .fold(0.0_f32, |peak, value| peak.max(value.abs()));
    assert!((peak - 0.891_250_9).abs() < 1e-6);

    let quiet = dir.path().join("quiet.wav");
    let mut writer = hound::WavWriter::create(&quiet, spec).unwrap();
    writer.write_sample(0.0_f32).unwrap();
    writer.write_sample(0.0_f32).unwrap();
    writer.finalize().unwrap();
    assert_eq!(
        normalize_wav_file(&quiet, 0.89).unwrap(),
        NormalizeOutcome::Silent
    );
}

#[test]
fn invalid_lengths_and_double_arming_are_rejected() {
    assert!(resolve_capture_frames(&request(ResampleStop::Bars(0), false), 0, CLOCK).is_err());
    assert!(
        resolve_capture_frames(&request(ResampleStop::Seconds(f32::NAN), false), 0, CLOCK).is_err()
    );
    let dir = tempfile::tempdir().unwrap();
    let (control, _tap, returns) = capture_channel();
    let mut resampler = Resampler::new(dir.path().to_path_buf(), RATE, control, returns);
    resampler
        .arm(request(ResampleStop::Bars(1), false), 0, CLOCK)
        .unwrap();
    assert!(resampler
        .arm(request(ResampleStop::Bars(1), false), 0, CLOCK)
        .is_err());
}

fn sample_track(id: u16, path: &str) -> TrackDefinition {
    let mut track = TrackDefinition::new(
        TrackId(id),
        format!("s{id}"),
        TrackKind::Sample,
        Pattern::new("p", 1, 0.0, 0, vec![None]).unwrap(),
    );
    track.sample = Some(SampleSettings::new(path, SampleMode::OneShot));
    track
}

#[test]
fn destinations_respect_track_limit_and_kind() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("a.wav");
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut writer = hound::WavWriter::create(&wav, spec).unwrap();
    for _ in 0..100 {
        writer.write_sample(0.5_f32).unwrap();
    }
    writer.finalize().unwrap();
    let mut bank = SampleAssetBank::new(usize::MAX);
    let asset = bank.load_file(&wav).unwrap();
    let context = shelloop::SampleContext {
        assets: &bank,
        base_dir: dir.path(),
    };
    let synth = TrackDefinition::new(
        TrackId(1),
        "synth",
        TrackKind::Synth,
        Pattern::new("p", 1, 0.0, 0, vec![None]).unwrap(),
    );
    let mut engine = MultiTrackEngine::new(RATE, 120.0, 4, 1, 4, vec![synth]).unwrap();
    for _ in 0..500 {
        engine.next_stereo_frame();
    }
    let settings = SampleSettings::new("a.wav", SampleMode::OneShot);
    let playback = CompiledSamplePlayback::new(bank.get(asset).unwrap(), &settings, RATE).unwrap();
    assert!(engine.replace_sample(TrackId(1), playback.clone()).is_err());

    for id in 2..=MAX_REALTIME_TRACKS as u16 {
        let prepared = PreparedTrack::new(
            RATE,
            120.0,
            4,
            1,
            4,
            sample_track(id, "a.wav"),
            Some(context),
        )
        .unwrap();
        engine
            .add_track(prepared)
            .map_err(|(error, _)| error)
            .unwrap();
        assert_eq!(engine.track_position(TrackId(id)), Some(500));
    }
    let extra = PreparedTrack::new(
        RATE,
        120.0,
        4,
        1,
        4,
        sample_track(99, "a.wav"),
        Some(context),
    )
    .unwrap();
    assert!(engine.add_track(extra).is_err());
    assert!(engine.replace_sample(TrackId(2), playback).is_ok());
    for _ in 0..100 {
        let (left, right) = engine.next_stereo_frame();
        assert!(left.is_finite() && right.is_finite());
    }
}

#[test]
fn commands_parse() {
    assert!(matches!(
        parse_resample_command("resample arm master bars 4").unwrap(),
        ResampleCommand::Arm {
            stop: ResampleStop::Bars(4),
            ..
        }
    ));
    assert!(matches!(
        parse_resample_command("resample arm track 2 bars 2").unwrap(),
        ResampleCommand::Arm {
            source: shelloop::ResampleSourceSpec::Track(Some(TrackId(2))),
            ..
        }
    ));
    assert_eq!(
        parse_resample_command("resample stop bar").unwrap(),
        ResampleCommand::Stop(QuantizeBoundary::Bar { beats_per_bar: 4 })
    );
    assert_eq!(
        parse_resample_command("resample normalize on").unwrap(),
        ResampleCommand::Normalize(true)
    );
    assert!(parse_resample_command("resample destination elsewhere").is_err());
}
