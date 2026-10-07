//! Spec 10 — internal resampling: capture Shelloop's own output and turn it
//! into a sample asset without stopping playback.
//!
//! The audio callback owns only a [`CaptureTap`], which writes frames into a
//! preallocated [`RealtimeRecordingProducer`] between two exact frames.
//! Arming, file naming, finalization, normalization and asset decoding run
//! on control/background threads through [`Resampler`].

use crate::{
    decode_wav_file, next_boundary_frame, spawn_realtime_recording, DecodedWav, QuantizeBoundary,
    RealtimeRecordingFinalizer, RealtimeRecordingProducer, RecordingSummary, TrackId,
    WavRecordingConfig,
};
use crossbeam_channel::{bounded, Receiver, Sender, TryRecvError};
use std::path::{Path, PathBuf};
use std::thread;

pub const RESAMPLE_BLOCK_FRAMES: usize = 1024;
pub const RESAMPLE_QUEUE_BLOCKS: usize = 128;
pub const MAX_RESAMPLE_BARS: u16 = 64;
pub const MAX_RESAMPLE_SECONDS: f32 = 300.0;
/// Peak normalization target (-1 dBFS).
pub const NORMALIZE_TARGET_PEAK: f32 = 0.891_250_9;
/// Captures quieter than this are reported as silent and not normalized.
pub const SILENCE_THRESHOLD: f32 = 1.0e-5;
const MAX_ASSET_BYTES: usize = 512 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResampleSource {
    /// Protected master output (post-protection, as with `--record`).
    Master,
    /// One track's post-fader output.
    Track(TrackId),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ResampleStop {
    Manual { boundary: QuantizeBoundary },
    Bars(u16),
    Seconds(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResampleDestination {
    AssetOnly,
    NewSampleTrack,
    ReplaceTrackAsset { track: TrackId },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResampleRequest {
    pub source: ResampleSource,
    pub start_boundary: QuantizeBoundary,
    pub stop: ResampleStop,
    pub normalize: bool,
    pub destination: ResampleDestination,
}

impl ResampleRequest {
    pub fn validate(&self) -> Result<(), String> {
        match self.stop {
            ResampleStop::Bars(bars) if !(1..=MAX_RESAMPLE_BARS).contains(&bars) => Err(format!(
                "resample length must be between 1 and {MAX_RESAMPLE_BARS} bars"
            )),
            ResampleStop::Seconds(seconds)
                if !seconds.is_finite() || seconds <= 0.0 || seconds > MAX_RESAMPLE_SECONDS =>
            {
                Err(format!(
                    "resample length must be between 0 and {MAX_RESAMPLE_SECONDS} seconds"
                ))
            }
            _ => Ok(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CaptureId(pub u64);

/// Musical clock used to resolve boundaries to frames.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureClock {
    pub sample_rate: u32,
    pub bpm: f64,
    pub steps_per_beat: u32,
    pub beats_per_bar: u32,
}

impl CaptureClock {
    pub fn frames_per_bar(&self) -> f64 {
        f64::from(self.sample_rate) * 60.0 / self.bpm * f64::from(self.beats_per_bar)
    }
}

/// Exact frame interval for a capture: `[start, stop)`.
pub fn resolve_capture_frames(
    request: &ResampleRequest,
    current_frame: u64,
    clock: CaptureClock,
) -> Result<(u64, Option<u64>), String> {
    request.validate()?;
    let start = next_boundary_frame(
        current_frame,
        clock.sample_rate,
        clock.bpm,
        clock.steps_per_beat,
        request.start_boundary,
    )?;
    let stop = match request.stop {
        ResampleStop::Manual { .. } => None,
        ResampleStop::Bars(bars) => {
            Some(start + (f64::from(bars) * clock.frames_per_bar()).round() as u64)
        }
        ResampleStop::Seconds(seconds) => {
            Some(start + (f64::from(seconds) * f64::from(clock.sample_rate)).round() as u64)
        }
    };
    Ok((start, stop))
}

/// Messages from the control thread to the callback-side tap.
pub enum CaptureControl {
    Start {
        id: CaptureId,
        source: ResampleSource,
        start_frame: u64,
        stop_frame: Option<u64>,
        producer: Box<RealtimeRecordingProducer>,
    },
    /// Stop at an exact frame (manual stop). Frames at or after it are excluded.
    StopAt {
        id: CaptureId,
        frame: u64,
    },
    Cancel {
        id: CaptureId,
    },
}

/// A producer handed back to the control thread when a capture ends, so it
/// is flushed and dropped off the audio thread.
pub struct CaptureReturn {
    pub id: CaptureId,
    pub producer: Box<RealtimeRecordingProducer>,
    pub frames: u64,
    pub cancelled: bool,
}

struct ActiveCapture {
    id: CaptureId,
    source: ResampleSource,
    start_frame: u64,
    stop_frame: Option<u64>,
    frames: u64,
    producer: Box<RealtimeRecordingProducer>,
}

/// Callback-side capture state. All methods are bounded and nonblocking;
/// none allocate.
pub struct CaptureTap {
    control: Receiver<CaptureControl>,
    returns: Sender<CaptureReturn>,
    active: Option<ActiveCapture>,
}

impl CaptureTap {
    /// Drain pending control messages; call once per callback or frame.
    pub fn poll_control(&mut self) {
        for _ in 0..4 {
            let Ok(message) = self.control.try_recv() else {
                return;
            };
            match message {
                CaptureControl::Start {
                    id,
                    source,
                    start_frame,
                    stop_frame,
                    producer,
                } => {
                    if let Some(previous) = self.active.take() {
                        self.finish(previous, true);
                    }
                    self.active = Some(ActiveCapture {
                        id,
                        source,
                        start_frame,
                        stop_frame,
                        frames: 0,
                        producer,
                    });
                }
                CaptureControl::StopAt { id, frame } => {
                    if let Some(active) = self.active.as_mut().filter(|active| active.id == id) {
                        active.stop_frame = Some(frame.max(active.start_frame));
                    }
                }
                CaptureControl::Cancel { id } => {
                    if self.active.as_ref().is_some_and(|active| active.id == id) {
                        let active = self.active.take().expect("checked above");
                        self.finish(active, true);
                    }
                }
            }
        }
    }

    pub fn is_capturing(&self) -> bool {
        self.active.is_some()
    }

    pub fn source(&self) -> Option<ResampleSource> {
        self.active.as_ref().map(|active| active.source)
    }

    /// Offer the frame rendered at `frame`. `master` is the protected master
    /// output and `track` the selected track's post-fader output (when the
    /// source is a track). Capture covers exactly `[start, stop)`.
    pub fn process_frame(&mut self, frame: u64, master: (f32, f32), track: Option<(f32, f32)>) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active.stop_frame.is_some_and(|stop| frame >= stop) {
            let active = self.active.take().expect("checked above");
            self.finish(active, false);
            return;
        }
        if frame < active.start_frame {
            return;
        }
        let (left, right) = match active.source {
            ResampleSource::Master => master,
            ResampleSource::Track(_) => track.unwrap_or((0.0, 0.0)),
        };
        active.producer.push_frame(&[left, right]);
        active.frames += 1;
    }

    fn finish(&mut self, active: ActiveCapture, cancelled: bool) {
        let returned = CaptureReturn {
            id: active.id,
            producer: active.producer,
            frames: active.frames,
            cancelled,
        };
        if let Err(error) = self.returns.try_send(returned) {
            // The return channel is sized for every possible capture, so this
            // only happens when the control side is gone. Leak rather than
            // free on the audio thread.
            std::mem::forget(error.into_inner());
        }
    }
}

/// Create the paired control sender and callback tap.
pub fn capture_channel() -> (Sender<CaptureControl>, CaptureTap, Receiver<CaptureReturn>) {
    let (control_sender, control_receiver) = bounded(4);
    let (return_sender, return_receiver) = bounded(8);
    (
        control_sender,
        CaptureTap {
            control: control_receiver,
            returns: return_sender,
            active: None,
        },
        return_receiver,
    )
}

#[derive(Debug, Clone, PartialEq)]
pub enum ResampleState {
    Idle,
    Armed {
        request: ResampleRequest,
        capture_id: CaptureId,
        start_frame: u64,
        stop_frame: Option<u64>,
    },
    Recording {
        capture_id: CaptureId,
    },
    Finalizing {
        capture_id: CaptureId,
    },
    Ready {
        path: PathBuf,
    },
    Failed {
        message: String,
    },
}

/// A finalized, decoded capture ready to be registered.
#[derive(Debug, Clone)]
pub struct FinishedCapture {
    pub capture_id: CaptureId,
    pub path: PathBuf,
    pub request: ResampleRequest,
    pub decoded: DecodedWav,
    pub summary: RecordingSummary,
    /// True when blocks were dropped; the audio has gaps.
    pub degraded: bool,
    pub silent: bool,
}

struct PendingCapture {
    id: CaptureId,
    request: ResampleRequest,
    temp_path: PathBuf,
    final_path: PathBuf,
    finalizer: Option<RealtimeRecordingFinalizer>,
}

type FinalizeResult = (CaptureId, Result<FinishedCapture, String>);

/// Control-thread resampling state machine.
pub struct Resampler {
    directory: PathBuf,
    sample_rate: u32,
    control: Sender<CaptureControl>,
    returns: Receiver<CaptureReturn>,
    state: ResampleState,
    pending: Option<PendingCapture>,
    finalize_results: (Sender<FinalizeResult>, Receiver<FinalizeResult>),
    next_id: u64,
    frame_hint: u64,
}

impl Resampler {
    pub fn new(
        directory: PathBuf,
        sample_rate: u32,
        control: Sender<CaptureControl>,
        returns: Receiver<CaptureReturn>,
    ) -> Self {
        Self {
            directory,
            sample_rate,
            control,
            returns,
            state: ResampleState::Idle,
            pending: None,
            finalize_results: bounded(4),
            next_id: 1,
            frame_hint: 0,
        }
    }

    pub fn state(&self) -> &ResampleState {
        &self.state
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn busy(&self) -> bool {
        matches!(
            self.state,
            ResampleState::Armed { .. }
                | ResampleState::Recording { .. }
                | ResampleState::Finalizing { .. }
        )
    }

    /// Arm a capture. The temporary file and writer thread are created here,
    /// off the audio thread; the callback only starts writing at the exact
    /// start frame.
    pub fn arm(
        &mut self,
        request: ResampleRequest,
        current_frame: u64,
        clock: CaptureClock,
    ) -> Result<(u64, Option<u64>), String> {
        if self.busy() {
            return Err("a resample capture is already in progress".into());
        }
        if clock.sample_rate != self.sample_rate {
            return Err("capture clock sample rate does not match the session".into());
        }
        let (start_frame, stop_frame) = resolve_capture_frames(&request, current_frame, clock)?;
        std::fs::create_dir_all(&self.directory)
            .map_err(|error| format!("create capture directory: {error}"))?;
        let final_path = next_resample_path(&self.directory)?;
        let temp_path = final_path.with_extension("wav.part");
        let config = WavRecordingConfig::new(self.sample_rate, 2, RESAMPLE_QUEUE_BLOCKS)?;
        let (producer, finalizer) =
            spawn_realtime_recording(&temp_path, config, RESAMPLE_BLOCK_FRAMES)?;
        let id = CaptureId(self.next_id);
        self.next_id += 1;
        let message = CaptureControl::Start {
            id,
            source: request.source,
            start_frame,
            stop_frame,
            producer: Box::new(producer),
        };
        if let Err(error) = self.control.try_send(message) {
            // Drop the producer and finish the writer so no partial file stays.
            drop(error.into_inner());
            let _ = finalizer.finish();
            let _ = std::fs::remove_file(&temp_path);
            return Err("resample control queue is full; try again".into());
        }
        self.pending = Some(PendingCapture {
            id,
            request,
            temp_path,
            final_path,
            finalizer: Some(finalizer),
        });
        self.state = ResampleState::Armed {
            request,
            capture_id: id,
            start_frame,
            stop_frame,
        };
        Ok((start_frame, stop_frame))
    }

    /// Request a manual stop on the given boundary.
    pub fn stop(
        &mut self,
        current_frame: u64,
        clock: CaptureClock,
        boundary: QuantizeBoundary,
    ) -> Result<u64, String> {
        let id = match &self.state {
            ResampleState::Armed { capture_id, .. } | ResampleState::Recording { capture_id } => {
                *capture_id
            }
            _ => return Err("no resample capture is armed or recording".into()),
        };
        let frame = next_boundary_frame(
            current_frame,
            clock.sample_rate,
            clock.bpm,
            clock.steps_per_beat,
            boundary,
        )?;
        self.control
            .try_send(CaptureControl::StopAt { id, frame })
            .map_err(|_| "resample control queue is full; try again".to_string())?;
        Ok(frame)
    }

    pub fn cancel(&mut self) -> Result<(), String> {
        let id = match &self.state {
            ResampleState::Armed { capture_id, .. } | ResampleState::Recording { capture_id } => {
                *capture_id
            }
            _ => return Err("no resample capture to cancel".into()),
        };
        self.control
            .try_send(CaptureControl::Cancel { id })
            .map_err(|_| "resample control queue is full; try again".to_string())
    }

    /// Clear a Ready/Failed state.
    pub fn acknowledge(&mut self) {
        if !self.busy() {
            self.state = ResampleState::Idle;
        }
    }

    /// Advance the state machine. Returns a capture when finalization and
    /// decoding succeeded; it is never returned before that.
    pub fn poll(&mut self, current_frame: u64) -> Option<FinishedCapture> {
        self.frame_hint = current_frame;
        if let ResampleState::Armed {
            capture_id,
            start_frame,
            ..
        } = self.state
        {
            if current_frame >= start_frame {
                self.state = ResampleState::Recording { capture_id };
            }
        }

        match self.returns.try_recv() {
            Ok(returned) => self.begin_finalize(returned),
            Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => {}
        }

        match self.finalize_results.1.try_recv() {
            Ok((_, Ok(finished))) => {
                self.state = ResampleState::Ready {
                    path: finished.path.clone(),
                };
                Some(finished)
            }
            Ok((_, Err(message))) => {
                self.state = ResampleState::Failed { message };
                None
            }
            Err(_) => None,
        }
    }

    fn begin_finalize(&mut self, returned: CaptureReturn) {
        let Some(mut pending) = self
            .pending
            .take()
            .filter(|pending| pending.id == returned.id)
        else {
            drop(returned.producer);
            return;
        };
        // Dropping the producer flushes its last partial block.
        drop(returned.producer);
        let finalizer = pending
            .finalizer
            .take()
            .expect("finalizer present until finalize");
        if returned.cancelled {
            let _ = finalizer.finish();
            let _ = std::fs::remove_file(&pending.temp_path);
            self.state = ResampleState::Idle;
            return;
        }
        self.state = ResampleState::Finalizing {
            capture_id: pending.id,
        };
        let sender = self.finalize_results.0.clone();
        let spawned = thread::Builder::new()
            .name("shelloop-resample-finalize".into())
            .spawn(move || {
                let id = pending.id;
                let result = finalize_capture(pending, finalizer);
                let _ = sender.send((id, result));
            });
        if let Err(error) = spawned {
            self.state = ResampleState::Failed {
                message: format!("failed to start resample finalizer: {error}"),
            };
        }
    }

    /// Bounded shutdown: cancels an active capture and waits briefly for
    /// finalization.
    pub fn shutdown(mut self) {
        let _ = self.cancel();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while self.busy() && std::time::Instant::now() < deadline {
            let _ = self.poll(self.frame_hint);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

fn finalize_capture(
    pending: PendingCapture,
    finalizer: RealtimeRecordingFinalizer,
) -> Result<FinishedCapture, String> {
    let cleanup = |message: String| {
        let _ = std::fs::remove_file(&pending.temp_path);
        Err(message)
    };
    let summary = match finalizer.finish() {
        Ok(summary) => summary,
        Err(error) => return cleanup(format!("resample write failed: {error}")),
    };
    if summary.frames_written == 0 {
        return cleanup("resample captured no audio".into());
    }
    let mut silent = false;
    if pending.request.normalize {
        match normalize_wav_file(&pending.temp_path, NORMALIZE_TARGET_PEAK) {
            Ok(NormalizeOutcome::Silent) => silent = true,
            Ok(NormalizeOutcome::Normalized { .. }) => {}
            Err(error) => return cleanup(error),
        }
    }
    if let Err(error) = std::fs::rename(&pending.temp_path, &pending.final_path) {
        return cleanup(format!("promote resample file: {error}"));
    }
    // A failed decode leaves the finalized WAV for recovery.
    let decoded = decode_wav_file(&pending.final_path, MAX_ASSET_BYTES)?;
    if !pending.request.normalize {
        silent = decoded.frames.iter().all(|frame| {
            frame.left.abs() < SILENCE_THRESHOLD && frame.right.abs() < SILENCE_THRESHOLD
        });
    }
    Ok(FinishedCapture {
        capture_id: pending.id,
        path: pending.final_path,
        request: pending.request,
        decoded,
        summary,
        degraded: summary.dropped_blocks > 0 || summary.rejected_blocks > 0,
        silent,
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NormalizeOutcome {
    Silent,
    Normalized { gain: f32 },
}

/// Offline peak normalization of a float WAV, rewriting it in place via a
/// temporary sibling.
pub fn normalize_wav_file(path: &Path, target_peak: f32) -> Result<NormalizeOutcome, String> {
    let mut reader =
        hound::WavReader::open(path).map_err(|error| format!("open capture: {error}"))?;
    let spec = reader.spec();
    if spec.sample_format != hound::SampleFormat::Float || spec.bits_per_sample != 32 {
        return Err("normalization expects a 32-bit float capture".into());
    }
    let samples = reader
        .samples::<f32>()
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("read capture: {error}"))?;
    let peak = samples
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    if peak < SILENCE_THRESHOLD {
        return Ok(NormalizeOutcome::Silent);
    }
    let gain = target_peak / peak;
    let temp = path.with_extension("norm.part");
    let result = (|| {
        let mut writer =
            hound::WavWriter::create(&temp, spec).map_err(|error| format!("normalize: {error}"))?;
        for sample in &samples {
            writer
                .write_sample(sample * gain)
                .map_err(|error| format!("normalize: {error}"))?;
        }
        writer
            .finalize()
            .map_err(|error| format!("normalize: {error}"))?;
        std::fs::rename(&temp, path).map_err(|error| format!("normalize: {error}"))
    })();
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    Ok(NormalizeOutcome::Normalized { gain })
}

/// Deterministic `Resample-0001.wav` style naming with collision increment.
pub fn next_resample_path(directory: &Path) -> Result<PathBuf, String> {
    for index in 1..=9_999 {
        let candidate = directory.join(format!("Resample-{index:04}.wav"));
        let part = candidate.with_extension("wav.part");
        if !candidate.exists() && !part.exists() {
            return Ok(candidate);
        }
    }
    Err("no free Resample-NNNN.wav name in the capture directory".into())
}

/// Parse `resample ...` commands into control actions.
#[derive(Debug, Clone, PartialEq)]
pub enum ResampleCommand {
    Arm {
        source: ResampleSourceSpec,
        stop: ResampleStop,
    },
    Start,
    Stop(QuantizeBoundary),
    Cancel,
    Normalize(bool),
    Destination(DestinationSpec),
    Status,
}

/// Source as typed; `Track(None)` means the selected track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResampleSourceSpec {
    Master,
    Track(Option<TrackId>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationSpec {
    AssetOnly,
    NewTrack,
    /// Replace the given (or selected) sample track's asset.
    Replace(Option<TrackId>),
}

pub fn parse_boundary(word: &str) -> Result<QuantizeBoundary, String> {
    match word {
        "now" | "immediate" => Ok(QuantizeBoundary::Immediate),
        "step" => Ok(QuantizeBoundary::Step),
        "beat" => Ok(QuantizeBoundary::Beat),
        "bar" => Ok(QuantizeBoundary::Bar { beats_per_bar: 4 }),
        other => Err(format!(
            "unknown boundary {other}; use now, step, beat or bar"
        )),
    }
}

pub fn parse_resample_command(line: &str) -> Result<ResampleCommand, String> {
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.first() != Some(&"resample") {
        return Err("expected resample <command>".into());
    }
    let parse_length = |kind: &str, value: Option<&&str>| -> Result<ResampleStop, String> {
        let value = value.ok_or("missing resample length")?;
        match kind {
            "bars" => value
                .parse::<u16>()
                .map(ResampleStop::Bars)
                .map_err(|_| "bars must be a whole number".to_string()),
            "seconds" | "secs" => value
                .parse::<f32>()
                .map(ResampleStop::Seconds)
                .map_err(|_| "seconds must be a number".to_string()),
            other => Err(format!("unknown length unit {other}; use bars or seconds")),
        }
    };
    match parts.get(1).copied() {
        Some("arm") => {
            let (source, rest) = match parts.get(2).copied() {
                Some("master") => (ResampleSourceSpec::Master, &parts[3..]),
                Some("track") => match parts.get(3).and_then(|value| value.parse::<u16>().ok()) {
                    Some(id) => (ResampleSourceSpec::Track(Some(TrackId(id))), &parts[4..]),
                    None => (ResampleSourceSpec::Track(None), &parts[3..]),
                },
                _ => {
                    return Err("expected resample arm master|track [ID] [bars N|seconds S]".into())
                }
            };
            let stop = match rest {
                [] => ResampleStop::Manual {
                    boundary: QuantizeBoundary::Bar { beats_per_bar: 4 },
                },
                [kind, value] => parse_length(kind, Some(value))?,
                _ => return Err("expected bars N or seconds S".into()),
            };
            Ok(ResampleCommand::Arm { source, stop })
        }
        Some("start") => Ok(ResampleCommand::Start),
        Some("stop") => Ok(ResampleCommand::Stop(match parts.get(2) {
            Some(word) => parse_boundary(word)?,
            None => QuantizeBoundary::Bar { beats_per_bar: 4 },
        })),
        Some("cancel") => Ok(ResampleCommand::Cancel),
        Some("normalize") => match parts.get(2).copied() {
            Some("on") => Ok(ResampleCommand::Normalize(true)),
            Some("off") => Ok(ResampleCommand::Normalize(false)),
            _ => Err("expected resample normalize on|off".into()),
        },
        Some("destination") => match parts.get(2).copied() {
            Some("asset") | Some("asset-only") => {
                Ok(ResampleCommand::Destination(DestinationSpec::AssetOnly))
            }
            Some("new-track") => Ok(ResampleCommand::Destination(DestinationSpec::NewTrack)),
            Some("replace") => Ok(ResampleCommand::Destination(DestinationSpec::Replace(
                parts
                    .get(3)
                    .and_then(|value| value.parse::<u16>().ok())
                    .map(TrackId),
            ))),
            _ => Err("expected resample destination asset|new-track|replace [TRACK]".into()),
        },
        Some("status") | None => Ok(ResampleCommand::Status),
        Some(other) => Err(format!("unknown resample command {other}")),
    }
}
