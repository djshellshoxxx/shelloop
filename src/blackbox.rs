//! Spec 11 — retrospective performance black box.
//!
//! Opt-in rolling capture of the protected stereo master plus normalized
//! control events. Nothing here runs unless [`spawn_black_box`] is called;
//! disabled mode allocates nothing and creates no files or directories.
//!
//! Thread layout:
//!
//! * **Audio callback** — owns a [`BlackBoxProducer`]. It fills fixed-size,
//!   preallocated blocks stamped with their first absolute frame and hands
//!   them over with nonblocking `try_send`. It never allocates, locks, blocks
//!   or performs IO. When no free block is available or the queue is full the
//!   frames are dropped and counted; playback is unaffected.
//! * **Collector thread** — owns the rolling audio ring (indexed by absolute
//!   frame), the bounded event ring, missing-range bookkeeping and save
//!   snapshots. It *polls* the audio queue (it never parks on it), so the
//!   producer's `try_send` never has to wake a sleeping receiver.
//! * **Writer thread** — encodes WAV/JSON, hashes and promotes files for one
//!   snapshot at a time, so disk latency never stalls audio collection.
//! * **Control threads** — use a [`BlackBoxHandle`]; every method is
//!   nonblocking except [`BlackBoxHandle::shutdown`], which waits for a
//!   caller-supplied bounded interval.
//!
//! Frame conventions: status `oldest_frame`/`newest_frame` are inclusive;
//! take/sidecar `start_frame` is inclusive and `end_frame` exclusive; missing
//! ranges are half-open `[start, end)`.

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError, Sender, TrySendError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, BufWriter, Read, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const MIN_BLACK_BOX_SECONDS: u32 = 1;
pub const MAX_BLACK_BOX_SECONDS: u32 = 120;
pub const DEFAULT_BLACK_BOX_MEMORY_BUDGET: usize = 128 * 1024 * 1024;
pub const DEFAULT_BLACK_BOX_DIRECTORY: &str = "shelloop-takes";
pub const BLACK_BOX_SIDECAR_SCHEMA: u32 = 1;
pub const MAX_BLACK_BOX_EVENTS: usize = 65_536;
/// Frames per producer block.
pub const BLACK_BOX_BLOCK_FRAMES: usize = 512;
/// Collector stall the block pool absorbs without dropping audio.
pub const BLACK_BOX_STALL_TOLERANCE_MS: u32 = 250;
/// Maximum save requests queued or in flight at once.
pub const MAX_PENDING_BLACK_BOX_SAVES: usize = 4;
/// Maximum UTF-8 bytes in any event string field.
pub const MAX_BLACK_BOX_EVENT_TEXT_BYTES: usize = 256;

const BYTES_PER_FRAME: usize = 8; // stereo f32
const CHANNELS: u16 = 2;
const MAX_SAMPLE_RATE: u32 = 768_000;
const EVENT_QUEUE_CAPACITY: usize = 4_096;
const CONTROL_QUEUE_CAPACITY: usize = 16;
const OUTCOME_QUEUE_CAPACITY: usize = 16;
const MAX_LOSS_RANGES: usize = 1_024;
const COLLECTOR_POLL: Duration = Duration::from_millis(5);
const MIN_POOL_BLOCKS: usize = 4;
const MAX_QUICK_SAVE_COUNTER: u32 = 9_999;

// ------------------------------------------------------------------ config

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlackBoxConfig {
    pub requested_seconds: u32,
    pub sample_rate: u32,
    /// Always 2 (interleaved stereo float32).
    pub channels: u16,
    /// Frames actually retained: min(requested × rate, memory budget).
    pub effective_frames: u64,
    /// Quick-save directory, created lazily on the first quick save.
    pub directory: PathBuf,
}

impl BlackBoxConfig {
    /// Validates bounds and computes retention under `memory_budget`, which
    /// covers the rolling ring (8 bytes/frame) plus the producer block pool.
    /// Creates nothing on disk.
    pub fn new(
        requested_seconds: u32,
        sample_rate: u32,
        directory: PathBuf,
        memory_budget: usize,
    ) -> Result<Self, String> {
        if !(MIN_BLACK_BOX_SECONDS..=MAX_BLACK_BOX_SECONDS).contains(&requested_seconds) {
            return Err(format!(
                "black box seconds must be between {MIN_BLACK_BOX_SECONDS} and \
                 {MAX_BLACK_BOX_SECONDS}, got {requested_seconds}"
            ));
        }
        if sample_rate == 0 || sample_rate > MAX_SAMPLE_RATE {
            return Err(format!(
                "black box sample rate must be between 1 and {MAX_SAMPLE_RATE} Hz, got {sample_rate}"
            ));
        }
        if directory.as_os_str().is_empty() {
            return Err("black box directory may not be empty".into());
        }

        let pool_bytes = pool_blocks_for(sample_rate, BLACK_BOX_BLOCK_FRAMES)
            * BLACK_BOX_BLOCK_FRAMES
            * BYTES_PER_FRAME;
        let one_second_bytes = sample_rate as usize * BYTES_PER_FRAME;
        let required = one_second_bytes + pool_bytes;
        if memory_budget < required {
            return Err(format!(
                "black box needs at least {required} bytes ({one_second_bytes} bytes for one \
                 second of stereo float32 at {sample_rate} Hz plus {pool_bytes} bytes of block \
                 pool) but only {memory_budget} bytes are available"
            ));
        }
        let budget_frames = ((memory_budget - pool_bytes) / BYTES_PER_FRAME) as u64;
        let requested_frames = u64::from(requested_seconds) * u64::from(sample_rate);

        Ok(Self {
            requested_seconds,
            sample_rate,
            channels: CHANNELS,
            effective_frames: requested_frames.min(budget_frames),
            directory,
        })
    }

    pub fn effective_seconds(&self) -> f64 {
        self.effective_frames as f64 / f64::from(self.sample_rate)
    }
}

fn pool_blocks_for(sample_rate: u32, block_frames: usize) -> usize {
    let stall_frames =
        (sample_rate as usize * BLACK_BOX_STALL_TOLERANCE_MS as usize).div_ceil(1_000);
    (stall_frames.div_ceil(block_frames.max(1)) + 2).max(MIN_POOL_BLOCKS)
}

// ------------------------------------------------------------------ events

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    Keyboard,
    Midi,
    Mouse,
    Cli,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ControlEventKind {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: f32,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
    XyMix {
        live_gain: f32,
        sequencer_gain: f32,
    },
    Transport {
        action: String,
    },
    Panic,
    EditorCommand {
        command: String,
    },
    RevisionQueued {
        track: u16,
        revision: u64,
        apply_at_frame: u64,
    },
    RevisionActivated {
        track: u16,
        revision: u64,
    },
    TempoChange {
        bpm: f64,
    },
    CaptureCommand {
        command: String,
    },
}

impl ControlEventKind {
    pub fn validate(&self) -> Result<(), String> {
        fn channel_ok(channel: u8) -> Result<(), String> {
            if channel < 16 {
                Ok(())
            } else {
                Err(format!("MIDI channel {channel} out of range 0-15"))
            }
        }
        fn data_ok(name: &str, value: u8) -> Result<(), String> {
            if value < 128 {
                Ok(())
            } else {
                Err(format!("{name} {value} out of range 0-127"))
            }
        }
        fn finite(name: &str, value: f64) -> Result<(), String> {
            if value.is_finite() {
                Ok(())
            } else {
                Err(format!("{name} must be finite"))
            }
        }
        fn text(name: &str, value: &str) -> Result<(), String> {
            if value.len() > MAX_BLACK_BOX_EVENT_TEXT_BYTES {
                Err(format!(
                    "{name} exceeds {MAX_BLACK_BOX_EVENT_TEXT_BYTES} bytes"
                ))
            } else if value.chars().any(char::is_control) {
                Err(format!("{name} contains control characters"))
            } else {
                Ok(())
            }
        }

        match self {
            Self::NoteOn {
                channel,
                note,
                velocity,
            } => {
                channel_ok(*channel)?;
                data_ok("note", *note)?;
                finite("velocity", f64::from(*velocity))
            }
            Self::NoteOff { channel, note } => {
                channel_ok(*channel)?;
                data_ok("note", *note)
            }
            Self::ControlChange {
                channel,
                controller,
                value,
            } => {
                channel_ok(*channel)?;
                data_ok("controller", *controller)?;
                data_ok("value", *value)
            }
            Self::XyMix {
                live_gain,
                sequencer_gain,
            } => {
                finite("live_gain", f64::from(*live_gain))?;
                finite("sequencer_gain", f64::from(*sequencer_gain))
            }
            Self::Transport { action } => text("transport action", action),
            Self::EditorCommand { command } => text("editor command", command),
            Self::CaptureCommand { command } => text("capture command", command),
            Self::TempoChange { bpm } => {
                finite("bpm", *bpm)?;
                if *bpm > 0.0 {
                    Ok(())
                } else {
                    Err("bpm must be positive".into())
                }
            }
            Self::Panic | Self::RevisionQueued { .. } | Self::RevisionActivated { .. } => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlEvent {
    pub sequence: u64,
    pub frame: u64,
    pub source: EventSource,
    #[serde(flatten)]
    pub kind: ControlEventKind,
}

impl ControlEvent {
    pub fn validate(&self) -> Result<(), String> {
        self.kind.validate()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionMetadata {
    pub session_id: String,
    pub engine_version: String,
    pub content_hash: Option<String>,
    pub bpm: Option<f64>,
    pub steps_per_beat: Option<u32>,
    pub seed: Option<u64>,
}

impl SessionMetadata {
    /// Metadata with this crate's version and no project facts.
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            engine_version: env!("CARGO_PKG_VERSION").to_string(),
            content_hash: None,
            bpm: None,
            steps_per_beat: None,
            seed: None,
        }
    }
}

/// A collision-resistant 16-hex-digit session id (time, pid and address entropy).
pub fn new_black_box_session_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let marker = 0_u8;
    let mut hasher = Sha256::new();
    hasher.update(nanos.to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    hasher.update((&marker as *const u8 as usize).to_le_bytes());
    hex(&hasher.finalize()[..8])
}

// ------------------------------------------------------------------ shared state

#[derive(Debug, Clone, PartialEq)]
pub struct BlackBoxStatus {
    pub armed: bool,
    pub effective_seconds: f64,
    /// Oldest retained frame (inclusive).
    pub oldest_frame: Option<u64>,
    /// Newest retained frame (inclusive).
    pub newest_frame: Option<u64>,
    pub dropped_audio_frames: u64,
    pub dropped_events: u64,
    pub rejected_events: u64,
    pub pending_saves: usize,
    pub failed: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveTarget {
    QuickSave,
    /// Explicit WAV path; `.wav` is appended when missing. Never overwritten.
    Path(PathBuf),
}

#[derive(Debug, Clone, PartialEq)]
pub struct SaveOutcome {
    pub request_id: u64,
    pub result: Result<SavedTake, String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedTake {
    pub wav_path: PathBuf,
    pub sidecar_path: PathBuf,
    /// First frame (inclusive).
    pub start_frame: u64,
    /// End frame (exclusive).
    pub end_frame: u64,
    pub complete: bool,
    /// Half-open missing audio ranges within the take.
    pub missing_ranges: Vec<(u64, u64)>,
}

#[derive(Debug, Default)]
struct PublishedStatus {
    armed: bool,
    oldest: Option<u64>,
    newest: Option<u64>,
    failed: Option<String>,
}

#[derive(Debug)]
struct Shared {
    dropped_audio_frames: AtomicU64,
    dropped_events: AtomicU64,
    rejected_events: AtomicU64,
    pending_saves: AtomicUsize,
    next_sequence: AtomicU64,
    next_request_id: AtomicU64,
    lost_event_count: AtomicU64,
    lost_event_min: AtomicU64,
    lost_event_max: AtomicU64,
    shutdown: AtomicBool,
    writer_busy: AtomicBool,
    status: Mutex<PublishedStatus>,
}

impl Shared {
    fn status(&self) -> MutexGuard<'_, PublishedStatus> {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn note_lost_event(&self, frame: u64) {
        self.dropped_events.fetch_add(1, Ordering::Relaxed);
        self.lost_event_min.fetch_min(frame, Ordering::Relaxed);
        self.lost_event_max.fetch_max(frame, Ordering::Relaxed);
        self.lost_event_count.fetch_add(1, Ordering::Release);
    }

    fn fail(&self, message: &str) {
        let mut status = self.status();
        status.armed = false;
        if status.failed.is_none() {
            status.failed = Some(message.to_string());
        }
    }
}

// ------------------------------------------------------------------ producer

#[derive(Debug)]
struct AudioBlock {
    start_frame: u64,
    frames: usize,
    samples: Box<[f32]>,
}

/// Audio-callback side. `push_frame` never allocates, locks, or blocks; it
/// fills a preallocated block and `try_send`s it; on failure it counts dropped
/// frames and continues.
#[derive(Debug)]
pub struct BlackBoxProducer {
    full_tx: Sender<AudioBlock>,
    free_rx: Receiver<AudioBlock>,
    current: Option<AudioBlock>,
    block_frames: usize,
    shared: Arc<Shared>,
}

impl BlackBoxProducer {
    /// Push one stereo frame stamped with its absolute output frame. Frames
    /// should be monotonic; forward jumps become recorded gaps and backward
    /// jumps start a new capture segment.
    pub fn push_frame(&mut self, frame: u64, left: f32, right: f32) {
        if let Some(block) = &self.current {
            if block.frames > 0 && block.start_frame.wrapping_add(block.frames as u64) != frame {
                self.submit();
            }
        }
        if self.current.is_none() {
            match self.free_rx.try_recv() {
                Ok(block) => self.current = Some(block),
                Err(_) => {
                    self.shared
                        .dropped_audio_frames
                        .fetch_add(1, Ordering::Relaxed);
                    return;
                }
            }
        }
        let Some(block) = self.current.as_mut() else {
            return;
        };
        if block.frames == 0 {
            block.start_frame = frame;
        }
        let index = block.frames * 2;
        if let Some(slot) = block.samples.get_mut(index..index + 2) {
            slot[0] = left;
            slot[1] = right;
            block.frames += 1;
        }
        if block.frames >= self.block_frames {
            self.submit();
        }
    }

    /// Push interleaved stereo samples whose first frame is `first_frame`.
    /// A trailing odd sample is ignored.
    pub fn push_interleaved(&mut self, first_frame: u64, samples: &[f32]) {
        for (offset, pair) in samples.chunks_exact(2).enumerate() {
            self.push_frame(first_frame.wrapping_add(offset as u64), pair[0], pair[1]);
        }
    }

    /// Hand a partially filled block to the collector (nonblocking).
    pub fn flush(&mut self) {
        self.submit();
    }

    pub fn dropped_frames(&self) -> u64 {
        self.shared.dropped_audio_frames.load(Ordering::Relaxed)
    }

    fn submit(&mut self) {
        let Some(block) = self.current.take() else {
            return;
        };
        if block.frames == 0 {
            self.current = Some(block);
            return;
        }
        match self.full_tx.try_send(block) {
            Ok(()) => {}
            Err(TrySendError::Full(mut block)) | Err(TrySendError::Disconnected(mut block)) => {
                self.shared
                    .dropped_audio_frames
                    .fetch_add(block.frames as u64, Ordering::Relaxed);
                block.frames = 0;
                self.current = Some(block);
            }
        }
    }
}

impl Drop for BlackBoxProducer {
    fn drop(&mut self) {
        self.submit();
    }
}

// ------------------------------------------------------------------ handle

enum ControlMessage {
    Save(SaveRequest),
    Clear,
}

struct SaveRequest {
    id: u64,
    target: SaveTarget,
    endpoint_frame: u64,
}

/// Control-thread handle; all methods nonblocking except `shutdown` (bounded).
#[derive(Debug)]
pub struct BlackBoxHandle {
    shared: Arc<Shared>,
    config: BlackBoxConfig,
    event_tx: Sender<ControlEvent>,
    control_tx: Sender<ControlMessage>,
    outcome_rx: Receiver<SaveOutcome>,
    done_rx: Receiver<()>,
    threads: Vec<JoinHandle<()>>,
}

impl std::fmt::Debug for ControlMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Save(request) => write!(formatter, "Save({})", request.id),
            Self::Clear => write!(formatter, "Clear"),
        }
    }
}

impl BlackBoxHandle {
    /// Assigns a monotonic sequence and enqueues the event. Returns false when
    /// the event is malformed (counted as rejected) or the queue is full or
    /// closed (counted as dropped and marks the frame as having lost events).
    pub fn record_event(&self, frame: u64, source: EventSource, kind: ControlEventKind) -> bool {
        if kind.validate().is_err() {
            self.shared.rejected_events.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        let event = ControlEvent {
            sequence: self.shared.next_sequence.fetch_add(1, Ordering::Relaxed),
            frame,
            source,
            kind,
        };
        match self.event_tx.try_send(event) {
            Ok(()) => true,
            Err(_) => {
                self.shared.note_lost_event(frame);
                false
            }
        }
    }

    /// Queue a save of the newest retained frames ending at or before
    /// `endpoint_frame` (exclusive). Returns the request id; the result arrives
    /// through [`Self::try_outcome`].
    pub fn request_save(&self, target: SaveTarget, endpoint_frame: u64) -> Result<u64, String> {
        if let Some(failed) = self.shared.status().failed.clone() {
            return Err(format!("black box unavailable: {failed}"));
        }
        let reserved = self.shared.pending_saves.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |pending| (pending < MAX_PENDING_BLACK_BOX_SAVES).then_some(pending + 1),
        );
        if reserved.is_err() {
            return Err(format!(
                "black box save queue is full ({MAX_PENDING_BLACK_BOX_SAVES} pending)"
            ));
        }
        let id = self.shared.next_request_id.fetch_add(1, Ordering::Relaxed) + 1;
        let message = ControlMessage::Save(SaveRequest {
            id,
            target,
            endpoint_frame,
        });
        match self.control_tx.try_send(message) {
            Ok(()) => Ok(id),
            Err(error) => {
                self.shared.pending_saves.fetch_sub(1, Ordering::AcqRel);
                Err(match error {
                    TrySendError::Full(_) => "black box control queue is full".into(),
                    TrySendError::Disconnected(_) => "black box collector has stopped".into(),
                })
            }
        }
    }

    /// Empty retained history on the collector thread; files are untouched.
    pub fn clear(&self) -> Result<(), String> {
        self.control_tx
            .try_send(ControlMessage::Clear)
            .map_err(|error| match error {
                TrySendError::Full(_) => "black box control queue is full".to_string(),
                TrySendError::Disconnected(_) => "black box collector has stopped".to_string(),
            })
    }

    pub fn try_outcome(&self) -> Option<SaveOutcome> {
        self.outcome_rx.try_recv().ok()
    }

    pub fn status(&self) -> BlackBoxStatus {
        let published = self.shared.status();
        BlackBoxStatus {
            armed: published.armed,
            effective_seconds: self.config.effective_seconds(),
            oldest_frame: published.oldest,
            newest_frame: published.newest,
            dropped_audio_frames: self.shared.dropped_audio_frames.load(Ordering::Relaxed),
            dropped_events: self.shared.dropped_events.load(Ordering::Relaxed),
            rejected_events: self.shared.rejected_events.load(Ordering::Relaxed),
            pending_saves: self.shared.pending_saves.load(Ordering::Acquire),
            failed: published.failed.clone(),
        }
    }

    pub fn config(&self) -> &BlackBoxConfig {
        &self.config
    }

    /// Stop the collector and writer. An in-flight save may finish; queued
    /// saves fail. Waits at most `timeout`; on timeout the threads are
    /// detached and an error is returned.
    pub fn shutdown(self, timeout: Duration) -> Result<(), String> {
        let BlackBoxHandle {
            shared,
            done_rx,
            threads,
            control_tx,
            event_tx,
            ..
        } = self;
        shared.shutdown.store(true, Ordering::Release);
        drop(control_tx);
        drop(event_tx);
        let deadline = Instant::now() + timeout;
        for _ in 0..threads.len() {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match done_rx.recv_timeout(remaining) {
                Ok(()) | Err(RecvTimeoutError::Disconnected) => {}
                Err(RecvTimeoutError::Timeout) => {
                    return Err(format!(
                        "black box did not stop within {} ms",
                        timeout.as_millis()
                    ));
                }
            }
        }
        let mut panicked = false;
        for thread in threads {
            panicked |= thread.join().is_err();
        }
        if panicked {
            return Err("black box worker thread panicked".into());
        }
        Ok(())
    }
}

/// Test hook: tuning knobs for block size, pool size and a stalled start.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlackBoxTuning {
    pub block_frames: usize,
    pub pool_blocks: usize,
    /// Collector waits until the returned gate is released or dropped.
    pub start_paused: bool,
}

/// Test hook: releases a collector started with `start_paused`.
#[doc(hidden)]
#[derive(Debug)]
pub struct BlackBoxGate {
    _release: Option<Sender<()>>,
}

impl BlackBoxGate {
    pub fn release(self) {}
}

pub fn spawn_black_box(
    config: BlackBoxConfig,
    metadata: SessionMetadata,
) -> Result<(BlackBoxProducer, BlackBoxHandle), String> {
    let tuning = BlackBoxTuning {
        block_frames: BLACK_BOX_BLOCK_FRAMES,
        pool_blocks: pool_blocks_for(config.sample_rate, BLACK_BOX_BLOCK_FRAMES),
        start_paused: false,
    };
    let (producer, handle, _gate) = spawn_black_box_tuned(config, metadata, tuning)?;
    Ok((producer, handle))
}

#[doc(hidden)]
pub fn spawn_black_box_tuned(
    config: BlackBoxConfig,
    metadata: SessionMetadata,
    tuning: BlackBoxTuning,
) -> Result<(BlackBoxProducer, BlackBoxHandle, BlackBoxGate), String> {
    if tuning.block_frames == 0 || tuning.pool_blocks == 0 {
        return Err("black box block size and pool must be greater than zero".into());
    }
    if config.channels != CHANNELS || config.effective_frames == 0 || config.sample_rate == 0 {
        return Err("black box config must come from BlackBoxConfig::new".into());
    }
    let ring_samples = usize::try_from(config.effective_frames)
        .ok()
        .and_then(|frames| frames.checked_mul(2))
        .ok_or_else(|| "black box ring size overflow".to_string())?;
    let block_samples = tuning
        .block_frames
        .checked_mul(2)
        .ok_or_else(|| "black box block size overflow".to_string())?;

    let (full_tx, full_rx) = bounded::<AudioBlock>(tuning.pool_blocks);
    let (free_tx, free_rx) = bounded::<AudioBlock>(tuning.pool_blocks);
    for _ in 0..tuning.pool_blocks {
        free_tx
            .try_send(AudioBlock {
                start_frame: 0,
                frames: 0,
                samples: vec![0.0; block_samples].into_boxed_slice(),
            })
            .map_err(|_| "failed to initialize black box block pool".to_string())?;
    }
    let ring = vec![0.0_f32; ring_samples];

    let (event_tx, event_rx) = bounded::<ControlEvent>(EVENT_QUEUE_CAPACITY);
    let (control_tx, control_rx) = bounded::<ControlMessage>(CONTROL_QUEUE_CAPACITY);
    let (outcome_tx, outcome_rx) = bounded::<SaveOutcome>(OUTCOME_QUEUE_CAPACITY);
    let (job_tx, job_rx) = bounded::<SaveJob>(1);
    let (done_tx, done_rx) = bounded::<()>(2);
    let (gate_tx, gate_rx) = if tuning.start_paused {
        let (tx, rx) = bounded::<()>(0);
        (Some(tx), Some(rx))
    } else {
        (None, None)
    };

    let shared = Arc::new(Shared {
        dropped_audio_frames: AtomicU64::new(0),
        dropped_events: AtomicU64::new(0),
        rejected_events: AtomicU64::new(0),
        pending_saves: AtomicUsize::new(0),
        next_sequence: AtomicU64::new(0),
        next_request_id: AtomicU64::new(0),
        lost_event_count: AtomicU64::new(0),
        lost_event_min: AtomicU64::new(u64::MAX),
        lost_event_max: AtomicU64::new(0),
        shutdown: AtomicBool::new(false),
        writer_busy: AtomicBool::new(false),
        status: Mutex::new(PublishedStatus {
            armed: true,
            ..PublishedStatus::default()
        }),
    });
    let metadata = Arc::new(metadata);
    let outbox = Outbox {
        tx: outcome_tx,
        rx: outcome_rx.clone(),
        shared: Arc::clone(&shared),
    };

    let writer = {
        let config = config.clone();
        let metadata = Arc::clone(&metadata);
        let outbox = outbox.clone();
        let shared = Arc::clone(&shared);
        let done = DoneSignal(done_tx.clone());
        thread::Builder::new()
            .name("shelloop-blackbox-writer".into())
            .spawn(move || {
                let _done = done;
                let mut writer = TakeWriter {
                    config,
                    metadata,
                    quick_counter: 1,
                };
                for job in job_rx {
                    let request_id = job.request_id;
                    let result = panic::catch_unwind(AssertUnwindSafe(|| writer.write(job)))
                        .unwrap_or_else(|_| Err("black box writer panicked".to_string()));
                    shared.writer_busy.store(false, Ordering::Release);
                    outbox.emit(SaveOutcome { request_id, result });
                }
            })
            .map_err(|error| format!("failed to start black box writer thread: {error}"))?
    };

    let collector = {
        let shared = Arc::clone(&shared);
        let cap = config.effective_frames;
        let done = DoneSignal(done_tx);
        thread::Builder::new()
            .name("shelloop-blackbox-collector".into())
            .spawn(move || {
                let _done = done;
                let mut collector = Collector {
                    cap,
                    ring,
                    history_start: None,
                    end: 0,
                    missing: VecDeque::new(),
                    event_losses: VecDeque::new(),
                    events: VecDeque::with_capacity(MAX_BLACK_BOX_EVENTS),
                    pending: VecDeque::new(),
                    shared: Arc::clone(&shared),
                    full_rx,
                    free_tx,
                    event_rx,
                    control_rx,
                    job_tx: Some(job_tx),
                    outbox,
                };
                let result =
                    panic::catch_unwind(AssertUnwindSafe(|| collector.run(gate_rx.as_ref())));
                if result.is_err() {
                    shared.fail("black box collector stopped unexpectedly");
                }
                collector.finish();
            })
            .map_err(|error| format!("failed to start black box collector thread: {error}"))?
    };

    Ok((
        BlackBoxProducer {
            full_tx,
            free_rx,
            current: None,
            block_frames: tuning.block_frames,
            shared: Arc::clone(&shared),
        },
        BlackBoxHandle {
            shared,
            config,
            event_tx,
            control_tx,
            outcome_rx,
            done_rx,
            threads: vec![collector, writer],
        },
        BlackBoxGate { _release: gate_tx },
    ))
}

struct DoneSignal(Sender<()>);

impl Drop for DoneSignal {
    fn drop(&mut self) {
        let _ = self.0.try_send(());
    }
}

#[derive(Clone)]
struct Outbox {
    tx: Sender<SaveOutcome>,
    rx: Receiver<SaveOutcome>,
    shared: Arc<Shared>,
}

impl Outbox {
    /// Deliver an outcome, discarding the oldest unread one when full.
    fn emit(&self, outcome: SaveOutcome) {
        let mut outcome = outcome;
        for _ in 0..=OUTCOME_QUEUE_CAPACITY {
            match self.tx.try_send(outcome) {
                Ok(()) => break,
                Err(TrySendError::Full(back)) => {
                    let _ = self.rx.try_recv();
                    outcome = back;
                }
                Err(TrySendError::Disconnected(_)) => break,
            }
        }
        let _ = self.shared.pending_saves.fetch_update(
            Ordering::AcqRel,
            Ordering::Acquire,
            |pending| pending.checked_sub(1),
        );
    }
}

// ------------------------------------------------------------------ collector

struct Collector {
    cap: u64,
    ring: Vec<f32>,
    /// First frame of the current history segment; `None` when empty.
    history_start: Option<u64>,
    /// Exclusive end frame of retained history.
    end: u64,
    missing: VecDeque<(u64, u64)>,
    event_losses: VecDeque<(u64, u64)>,
    events: VecDeque<ControlEvent>,
    pending: VecDeque<ControlMessage>,
    shared: Arc<Shared>,
    full_rx: Receiver<AudioBlock>,
    free_tx: Sender<AudioBlock>,
    event_rx: Receiver<ControlEvent>,
    control_rx: Receiver<ControlMessage>,
    job_tx: Option<Sender<SaveJob>>,
    outbox: Outbox,
}

struct SaveJob {
    request_id: u64,
    target: SaveTarget,
    start_frame: u64,
    end_frame: u64,
    save_request_frame: u64,
    samples: Vec<f32>,
    missing_audio: Vec<(u64, u64)>,
    missing_events: Vec<(u64, u64)>,
    events: Vec<ControlEvent>,
    dropped_audio_frames: u64,
    dropped_events: u64,
    rejected_events: u64,
}

impl Collector {
    fn stopping(&self) -> bool {
        self.shared.shutdown.load(Ordering::Acquire)
    }

    fn run(&mut self, gate: Option<&Receiver<()>>) {
        if let Some(gate) = gate {
            loop {
                if self.stopping() {
                    return;
                }
                match gate.recv_timeout(COLLECTOR_POLL) {
                    Err(RecvTimeoutError::Timeout) => {}
                    Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        }
        loop {
            self.drain_audio();
            self.drain_events();
            self.process_control();
            self.publish();
            if self.stopping() {
                return;
            }
            match self.control_rx.recv_timeout(COLLECTOR_POLL) {
                Ok(message) => self.pending.push_back(message),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    /// Fail queued saves and mark the black box disarmed.
    fn finish(&mut self) {
        self.job_tx = None;
        while let Ok(message) = self.control_rx.try_recv() {
            self.pending.push_back(message);
        }
        while let Some(message) = self.pending.pop_front() {
            if let ControlMessage::Save(request) = message {
                self.outbox.emit(SaveOutcome {
                    request_id: request.id,
                    result: Err("black box stopped before the save ran".into()),
                });
            }
        }
        self.shared.status().armed = false;
    }

    fn publish(&self) {
        let oldest = self.oldest();
        let mut status = self.shared.status();
        status.oldest = oldest;
        status.newest = oldest.map(|_| self.end - 1);
    }

    fn oldest(&self) -> Option<u64> {
        self.history_start
            .map(|start| start.max(self.end.saturating_sub(self.cap)))
            .filter(|oldest| *oldest < self.end)
    }

    fn reset_history(&mut self) {
        self.history_start = None;
        self.end = 0;
        self.missing.clear();
        self.event_losses.clear();
        self.events.clear();
    }

    fn drain_audio(&mut self) {
        while let Ok(mut block) = self.full_rx.try_recv() {
            self.ingest(&block);
            block.frames = 0;
            let _ = self.free_tx.try_send(block);
        }
    }

    fn ingest(&mut self, block: &AudioBlock) {
        let frames = block.frames.min(block.samples.len() / 2);
        if frames == 0 {
            return;
        }
        let start = block.start_frame;
        match self.history_start {
            None => {
                self.history_start = Some(start);
                self.end = start;
            }
            Some(_) if start < self.end => {
                // Frame clock went backwards: start a new capture segment.
                self.reset_history();
                self.history_start = Some(start);
                self.end = start;
            }
            Some(_) if start > self.end => {
                push_loss_range(&mut self.missing, (self.end, start));
                // Never leave stale audio in the gap; it is declared missing.
                let zero_from = self.end.max(start.saturating_sub(self.cap));
                self.zero_ring(zero_from, start - zero_from);
                self.end = start;
            }
            Some(_) => {}
        }
        let skip = (frames as u64).saturating_sub(self.cap) as usize;
        let samples = &block.samples[skip * 2..frames * 2];
        self.write_ring(start + skip as u64, samples);
        self.end = start + frames as u64;
        self.prune();
    }

    fn write_ring(&mut self, first_frame: u64, samples: &[f32]) {
        let frames = samples.len() / 2;
        let cap = self.cap as usize;
        let mut offset = 0;
        while offset < frames {
            let slot = ((first_frame + offset as u64) % self.cap) as usize;
            let run = (cap - slot).min(frames - offset);
            self.ring[slot * 2..(slot + run) * 2]
                .copy_from_slice(&samples[offset * 2..(offset + run) * 2]);
            offset += run;
        }
    }

    fn zero_ring(&mut self, first_frame: u64, frames: u64) {
        let cap = self.cap as usize;
        let frames = frames.min(self.cap) as usize;
        let mut offset = 0;
        while offset < frames {
            let slot = ((first_frame + offset as u64) % self.cap) as usize;
            let run = (cap - slot).min(frames - offset);
            self.ring[slot * 2..(slot + run) * 2].fill(0.0);
            offset += run;
        }
    }

    fn read_ring(&self, start: u64, end: u64) -> Vec<f32> {
        let frames = (end - start) as usize;
        let cap = self.cap as usize;
        let mut samples = Vec::with_capacity(frames * 2);
        let mut offset = 0;
        while offset < frames {
            let slot = ((start + offset as u64) % self.cap) as usize;
            let run = (cap - slot).min(frames - offset);
            samples.extend_from_slice(&self.ring[slot * 2..(slot + run) * 2]);
            offset += run;
        }
        samples
    }

    fn prune(&mut self) {
        let Some(oldest) = self.oldest() else {
            return;
        };
        prune_ranges(&mut self.missing, oldest);
        prune_ranges(&mut self.event_losses, oldest);
        while self
            .events
            .front()
            .is_some_and(|event| event.frame < oldest)
        {
            self.events.pop_front();
        }
    }

    fn drain_events(&mut self) {
        if self.shared.lost_event_count.swap(0, Ordering::Acquire) > 0 {
            let min = self.shared.lost_event_min.swap(u64::MAX, Ordering::Relaxed);
            let max = self.shared.lost_event_max.swap(0, Ordering::Relaxed);
            if min <= max {
                push_loss_range(&mut self.event_losses, (min, max.saturating_add(1)));
            }
        }
        while let Ok(event) = self.event_rx.try_recv() {
            if self.oldest().is_some_and(|oldest| event.frame < oldest) {
                continue; // already outside the retained window
            }
            if self.events.len() >= MAX_BLACK_BOX_EVENTS {
                if let Some(evicted) = self.events.pop_front() {
                    self.shared.dropped_events.fetch_add(1, Ordering::Relaxed);
                    push_loss_range(
                        &mut self.event_losses,
                        (evicted.frame, evicted.frame.saturating_add(1)),
                    );
                }
            }
            self.events.push_back(event);
        }
    }

    fn process_control(&mut self) {
        while let Ok(message) = self.control_rx.try_recv() {
            self.pending.push_back(message);
        }
        // Requests run in order; a save waits while the writer is busy.
        while let Some(front) = self.pending.front() {
            match front {
                ControlMessage::Clear => {
                    self.pending.pop_front();
                    self.reset_history();
                }
                ControlMessage::Save(_) => {
                    if self.shared.writer_busy.load(Ordering::Acquire) {
                        break;
                    }
                    let Some(ControlMessage::Save(request)) = self.pending.pop_front() else {
                        break;
                    };
                    self.start_save(request);
                }
            }
        }
    }

    fn start_save(&mut self, request: SaveRequest) {
        let id = request.id;
        let job = match self.snapshot(request) {
            Ok(job) => job,
            Err(error) => {
                self.outbox.emit(SaveOutcome {
                    request_id: id,
                    result: Err(error),
                });
                return;
            }
        };
        self.shared.writer_busy.store(true, Ordering::Release);
        let sent = self
            .job_tx
            .as_ref()
            .is_some_and(|tx| tx.try_send(job).is_ok());
        if !sent {
            self.shared.writer_busy.store(false, Ordering::Release);
            self.outbox.emit(SaveOutcome {
                request_id: id,
                result: Err("black box writer is unavailable".into()),
            });
        }
    }

    fn snapshot(&self, request: SaveRequest) -> Result<SaveJob, String> {
        let oldest = self
            .oldest()
            .ok_or_else(|| "black box has no captured audio yet".to_string())?;
        let end = request.endpoint_frame.min(self.end);
        if end <= oldest {
            return Err(format!(
                "save endpoint frame {} precedes retained history (oldest frame {oldest})",
                request.endpoint_frame
            ));
        }
        let start = oldest;
        let samples = self.read_ring(start, end);
        let missing_audio = intersect_ranges(&self.missing, start, end);
        let missing_events = intersect_ranges(&self.event_losses, start, end);
        let mut events: Vec<ControlEvent> = self
            .events
            .iter()
            .filter(|event| (start..end).contains(&event.frame))
            .cloned()
            .collect();
        events.sort_by_key(|event| (event.frame, event.sequence));

        Ok(SaveJob {
            request_id: request.id,
            target: request.target,
            start_frame: start,
            end_frame: end,
            save_request_frame: request.endpoint_frame,
            samples,
            missing_audio,
            missing_events,
            events,
            dropped_audio_frames: self.shared.dropped_audio_frames.load(Ordering::Relaxed),
            dropped_events: self.shared.dropped_events.load(Ordering::Relaxed),
            rejected_events: self.shared.rejected_events.load(Ordering::Relaxed),
        })
    }
}

/// Append a half-open range, merging with the last one when adjacent. Beyond
/// the bound the two oldest ranges merge, over-reporting rather than hiding loss.
fn push_loss_range(ranges: &mut VecDeque<(u64, u64)>, range: (u64, u64)) {
    if range.0 >= range.1 {
        return;
    }
    if let Some(last) = ranges.back_mut() {
        if range.0 <= last.1 && range.1 >= last.0 {
            last.0 = last.0.min(range.0);
            last.1 = last.1.max(range.1);
            return;
        }
    }
    ranges.push_back(range);
    if ranges.len() > MAX_LOSS_RANGES {
        if let (Some(first), Some(second)) = (ranges.pop_front(), ranges.pop_front()) {
            ranges.push_front((first.0.min(second.0), first.1.max(second.1)));
        }
    }
}

fn prune_ranges(ranges: &mut VecDeque<(u64, u64)>, oldest: u64) {
    ranges.retain(|range| range.1 > oldest);
    for range in ranges.iter_mut() {
        range.0 = range.0.max(oldest);
    }
}

fn intersect_ranges(ranges: &VecDeque<(u64, u64)>, start: u64, end: u64) -> Vec<(u64, u64)> {
    let mut result: Vec<(u64, u64)> = ranges
        .iter()
        .map(|range| (range.0.max(start), range.1.min(end)))
        .filter(|range| range.0 < range.1)
        .collect();
    result.sort_unstable();
    result
}

// ------------------------------------------------------------------ writer

#[derive(Serialize)]
struct Sidecar<'a> {
    schema: u32,
    engine_version: &'a str,
    session_id: &'a str,
    request_id: u64,
    sample_rate: u32,
    channels: u16,
    sample_format: &'static str,
    requested_seconds: u32,
    effective_seconds: f64,
    start_frame: u64,
    end_frame: u64,
    frame_count: u64,
    save_request_frame: u64,
    content_hash: Option<&'a str>,
    bpm: Option<f64>,
    steps_per_beat: Option<u32>,
    seed: Option<u64>,
    complete: bool,
    missing_audio_ranges: &'a [(u64, u64)],
    missing_audio_frames: u64,
    missing_event_ranges: &'a [(u64, u64)],
    dropped_audio_frames: u64,
    dropped_events: u64,
    rejected_events: u64,
    wav_file: String,
    wav_sha256: &'a str,
    events: &'a [ControlEvent],
}

struct TakeWriter {
    config: BlackBoxConfig,
    metadata: Arc<SessionMetadata>,
    quick_counter: u32,
}

impl TakeWriter {
    fn write(&mut self, job: SaveJob) -> Result<SavedTake, String> {
        let (wav_path, sidecar_path) = self.resolve_target(&job.target)?;
        let wav_tmp = temporary_sibling(&wav_path);
        let sidecar_tmp = temporary_sibling(&sidecar_path);
        let result = self.write_files(&job, &wav_path, &sidecar_path, &wav_tmp, &sidecar_tmp);
        // Temporaries never outlive a save, successful or not.
        let _ = fs::remove_file(&wav_tmp);
        let _ = fs::remove_file(&sidecar_tmp);
        result?;
        if matches!(job.target, SaveTarget::QuickSave) {
            self.quick_counter = self.quick_counter.saturating_add(1);
        }
        Ok(SavedTake {
            wav_path,
            sidecar_path,
            start_frame: job.start_frame,
            end_frame: job.end_frame,
            complete: job.missing_audio.is_empty() && job.missing_events.is_empty(),
            missing_ranges: job.missing_audio,
        })
    }

    fn resolve_target(&mut self, target: &SaveTarget) -> Result<(PathBuf, PathBuf), String> {
        match target {
            SaveTarget::Path(path) => {
                if path.as_os_str().is_empty() {
                    return Err("save path may not be empty".into());
                }
                let is_wav = path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("wav"));
                let wav = if is_wav {
                    path.clone()
                } else {
                    let mut name = path.as_os_str().to_owned();
                    name.push(".wav");
                    PathBuf::from(name)
                };
                let sidecar = wav.with_extension("json");
                for candidate in [&wav, &sidecar] {
                    if fs::symlink_metadata(candidate).is_ok() {
                        return Err(format!(
                            "refusing to overwrite existing file {}",
                            candidate.display()
                        ));
                    }
                }
                Ok((wav, sidecar))
            }
            SaveTarget::QuickSave => {
                let directory = &self.config.directory;
                fs::create_dir_all(directory).map_err(|error| {
                    format!(
                        "failed to create take directory {}: {error}",
                        directory.display()
                    )
                })?;
                let now = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or_default();
                let suffix = session_suffix(&self.metadata.session_id);
                while self.quick_counter <= MAX_QUICK_SAVE_COUNTER {
                    let stem = quick_save_file_stem(now, &suffix, self.quick_counter);
                    let wav = directory.join(format!("{stem}.wav"));
                    let sidecar = directory.join(format!("{stem}.json"));
                    if fs::symlink_metadata(&wav).is_err()
                        && fs::symlink_metadata(&sidecar).is_err()
                    {
                        return Ok((wav, sidecar));
                    }
                    self.quick_counter += 1;
                }
                self.quick_counter = 1;
                Err("no free quick-save file name is available".into())
            }
        }
    }

    fn write_files(
        &self,
        job: &SaveJob,
        wav_path: &Path,
        sidecar_path: &Path,
        wav_tmp: &Path,
        sidecar_tmp: &Path,
    ) -> Result<(), String> {
        write_wav(wav_tmp, self.config.sample_rate, &job.samples)
            .map_err(|error| format!("failed to write {}: {error}", wav_tmp.display()))?;
        let wav_sha256 = sha256_file(wav_tmp)
            .map_err(|error| format!("failed to hash {}: {error}", wav_tmp.display()))?;

        let metadata = &self.metadata;
        let missing_audio_frames = job
            .missing_audio
            .iter()
            .map(|range| range.1 - range.0)
            .sum();
        let sidecar = Sidecar {
            schema: BLACK_BOX_SIDECAR_SCHEMA,
            engine_version: &metadata.engine_version,
            session_id: &metadata.session_id,
            request_id: job.request_id,
            sample_rate: self.config.sample_rate,
            channels: self.config.channels,
            sample_format: "float32",
            requested_seconds: self.config.requested_seconds,
            effective_seconds: self.config.effective_seconds(),
            start_frame: job.start_frame,
            end_frame: job.end_frame,
            frame_count: job.end_frame - job.start_frame,
            save_request_frame: job.save_request_frame,
            content_hash: metadata.content_hash.as_deref(),
            bpm: metadata.bpm,
            steps_per_beat: metadata.steps_per_beat,
            seed: metadata.seed,
            complete: job.missing_audio.is_empty() && job.missing_events.is_empty(),
            missing_audio_ranges: &job.missing_audio,
            missing_audio_frames,
            missing_event_ranges: &job.missing_events,
            dropped_audio_frames: job.dropped_audio_frames,
            dropped_events: job.dropped_events,
            rejected_events: job.rejected_events,
            wav_file: wav_path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            wav_sha256: &wav_sha256,
            events: &job.events,
        };
        write_json(sidecar_tmp, &sidecar)
            .map_err(|error| format!("failed to write {}: {error}", sidecar_tmp.display()))?;

        // WAV first, sidecar last: the sidecar marks a complete take.
        promote(wav_tmp, wav_path)?;
        if let Err(error) = promote(sidecar_tmp, sidecar_path) {
            return match fs::remove_file(wav_path) {
                Ok(()) => Err(error),
                Err(remove_error) => Err(format!(
                    "{error}; partial take left at {} ({remove_error})",
                    wav_path.display()
                )),
            };
        }
        sync_parent(wav_path);
        Ok(())
    }
}

fn write_wav(path: &Path, sample_rate: u32, samples: &[f32]) -> io::Result<()> {
    let spec = hound::WavSpec {
        channels: CHANNELS,
        sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };
    let mut file = BufWriter::new(File::create(path)?);
    {
        let mut writer = hound::WavWriter::new(&mut file, spec).map_err(hound_error)?;
        for &sample in samples {
            writer.write_sample(sample).map_err(hound_error)?;
        }
        writer.finalize().map_err(hound_error)?;
    }
    let file = file.into_inner().map_err(|error| error.into_error())?;
    file.sync_all()
}

fn hound_error(error: hound::Error) -> io::Error {
    match error {
        hound::Error::IoError(error) => error,
        other => io::Error::other(other.to_string()),
    }
}

fn write_json(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut file = BufWriter::new(File::create(path)?);
    serde_json::to_writer_pretty(&mut file, value).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    let file = file.into_inner().map_err(|error| error.into_error())?;
    file.sync_all()
}

fn sha256_file(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

fn temporary_sibling(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Move `tmp` to `target` without ever replacing an existing file.
fn promote(tmp: &Path, target: &Path) -> Result<(), String> {
    match fs::hard_link(tmp, target) {
        Ok(()) => {
            let _ = fs::remove_file(tmp);
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(format!(
            "refusing to overwrite existing file {}",
            target.display()
        )),
        Err(_) => {
            // Filesystem without hard links: check, then rename.
            if fs::symlink_metadata(target).is_ok() {
                return Err(format!(
                    "refusing to overwrite existing file {}",
                    target.display()
                ));
            }
            fs::rename(tmp, target)
                .map_err(|error| format!("failed to finalize {}: {error}", target.display()))
        }
    }
}

fn sync_parent(path: &Path) {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

fn session_suffix(session_id: &str) -> String {
    let suffix: String = session_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect();
    if suffix.is_empty() {
        "session".into()
    } else {
        suffix.to_ascii_lowercase()
    }
}

/// Quick-save file stem in UTC, e.g. `shelloop-take-20261007-142530Z-abc123-01`.
pub fn quick_save_file_stem(unix_secs: u64, session_suffix: &str, counter: u32) -> String {
    let days = (unix_secs / 86_400) as i64;
    let seconds_of_day = unix_secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "shelloop-take-{year:04}{month:02}{day:02}-{:02}{:02}{:02}Z-{session_suffix}-{counter:02}",
        seconds_of_day / 3_600,
        (seconds_of_day / 60) % 60,
        seconds_of_day % 60
    )
}

/// Days since 1970-01-01 to proleptic Gregorian (year, month, day).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loss_ranges_merge_and_stay_bounded() {
        let mut ranges = VecDeque::new();
        push_loss_range(&mut ranges, (10, 20));
        push_loss_range(&mut ranges, (20, 25));
        assert_eq!(ranges, VecDeque::from([(10, 25)]));
        for index in 0..(MAX_LOSS_RANGES as u64 * 2) {
            push_loss_range(&mut ranges, (100 + index * 10, 105 + index * 10));
        }
        assert!(ranges.len() <= MAX_LOSS_RANGES);
        assert_eq!(ranges.front().map(|range| range.0), Some(10));
        prune_ranges(&mut ranges, 112);
        assert_eq!(ranges.front().map(|range| range.0), Some(112));
    }

    #[test]
    fn pool_covers_stall_tolerance() {
        let blocks = pool_blocks_for(48_000, BLACK_BOX_BLOCK_FRAMES);
        assert!(blocks * BLACK_BOX_BLOCK_FRAMES >= 12_000);
        assert!(pool_blocks_for(1, 512) >= MIN_POOL_BLOCKS);
    }

    #[test]
    fn session_suffix_is_filename_safe() {
        assert_eq!(session_suffix("AB/../c d-e"), "abcde");
        assert_eq!(session_suffix("///"), "session");
    }
}
