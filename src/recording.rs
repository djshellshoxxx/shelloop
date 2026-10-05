use crate::sanitize_sample;
use crossbeam_channel::{bounded, Sender, TrySendError};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

#[derive(Debug)]
pub struct RecordingQueue {
    capacity_blocks: usize,
    blocks: VecDeque<Vec<f32>>,
    dropped_blocks: u64,
}

impl RecordingQueue {
    pub fn new(capacity_blocks: usize) -> Self {
        Self {
            capacity_blocks,
            blocks: VecDeque::with_capacity(capacity_blocks),
            dropped_blocks: 0,
        }
    }

    pub fn push(&mut self, block: Vec<f32>) -> bool {
        if self.blocks.len() >= self.capacity_blocks {
            self.dropped_blocks = self.dropped_blocks.saturating_add(1);
            return false;
        }
        self.blocks.push_back(block);
        true
    }

    pub fn pop(&mut self) -> Option<Vec<f32>> {
        self.blocks.pop_front()
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn capacity_blocks(&self) -> usize {
        self.capacity_blocks
    }

    pub fn dropped_blocks(&self) -> u64 {
        self.dropped_blocks
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavRecordingConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub capacity_blocks: usize,
}

impl WavRecordingConfig {
    pub fn new(sample_rate: u32, channels: u16, capacity_blocks: usize) -> Result<Self, String> {
        if sample_rate == 0 {
            return Err("recording sample rate must be greater than zero".into());
        }
        if !(1..=64).contains(&channels) {
            return Err("recording channel count must be between 1 and 64".into());
        }
        if capacity_blocks == 0 {
            return Err("recording queue capacity must be greater than zero".into());
        }

        Ok(Self {
            sample_rate,
            channels,
            capacity_blocks,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordingSummary {
    pub sample_rate: u32,
    pub channels: u16,
    pub frames_written: u64,
    pub samples_written: u64,
    pub dropped_blocks: u64,
    pub rejected_blocks: u64,
}

pub struct RecordingWriter {
    sender: Sender<Vec<f32>>,
    worker: JoinHandle<Result<RecordingSummary, String>>,
    channels: usize,
    dropped_blocks: Arc<AtomicU64>,
    rejected_blocks: Arc<AtomicU64>,
}

impl RecordingWriter {
    pub fn spawn(path: impl AsRef<Path>, config: WavRecordingConfig) -> Result<Self, String> {
        let path = path.as_ref();
        if path.as_os_str().is_empty() {
            return Err("recording path may not be empty".into());
        }

        let path = path.to_path_buf();
        let (sender, receiver) = bounded::<Vec<f32>>(config.capacity_blocks);
        let dropped_blocks = Arc::new(AtomicU64::new(0));
        let rejected_blocks = Arc::new(AtomicU64::new(0));
        let worker_dropped = Arc::clone(&dropped_blocks);
        let worker_rejected = Arc::clone(&rejected_blocks);

        let worker = thread::Builder::new()
            .name("shelloop-wav-writer".into())
            .spawn(move || {
                let spec = hound::WavSpec {
                    channels: config.channels,
                    sample_rate: config.sample_rate,
                    bits_per_sample: 32,
                    sample_format: hound::SampleFormat::Float,
                };
                let mut writer = hound::WavWriter::create(&path, spec)
                    .map_err(|error| format!("failed to create recording {:?}: {error}", path))?;
                let mut samples_written = 0_u64;

                for block in receiver {
                    for sample in block {
                        writer
                            .write_sample(sanitize_sample(sample))
                            .map_err(|error| {
                                format!("failed to write recording sample: {error}")
                            })?;
                        samples_written = samples_written.saturating_add(1);
                    }
                }

                writer
                    .finalize()
                    .map_err(|error| format!("failed to finalize recording: {error}"))?;

                Ok(RecordingSummary {
                    sample_rate: config.sample_rate,
                    channels: config.channels,
                    frames_written: samples_written / u64::from(config.channels),
                    samples_written,
                    dropped_blocks: worker_dropped.load(Ordering::Relaxed),
                    rejected_blocks: worker_rejected.load(Ordering::Relaxed),
                })
            })
            .map_err(|error| format!("failed to start recording writer thread: {error}"))?;

        Ok(Self {
            sender,
            worker,
            channels: usize::from(config.channels),
            dropped_blocks,
            rejected_blocks,
        })
    }

    pub fn try_record(&self, block: Vec<f32>) -> bool {
        if block.len() % self.channels != 0 {
            self.rejected_blocks.fetch_add(1, Ordering::Relaxed);
            return false;
        }

        match self.sender.try_send(block) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped_blocks.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    pub fn dropped_blocks(&self) -> u64 {
        self.dropped_blocks.load(Ordering::Relaxed)
    }

    pub fn rejected_blocks(&self) -> u64 {
        self.rejected_blocks.load(Ordering::Relaxed)
    }

    pub fn finish(self) -> Result<RecordingSummary, String> {
        let RecordingWriter { sender, worker, .. } = self;
        drop(sender);
        worker
            .join()
            .map_err(|_| "recording writer thread panicked".to_string())?
    }
}
