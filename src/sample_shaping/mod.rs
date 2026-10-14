//! Spec 14 — sample shaping and slice playback.
//!
//! * [`slices`] — pure slice model, validation, equal division and offline
//!   transient detection (no audio-thread code).
//! * [`shaping`] — serialisable shaping settings, their `Copy` compiled form
//!   and the bounded per-voice DSP (filter, drive, transient shaper).
//! * [`command`] — typed `sample ...` / `slice ...` text commands.
//!
//! `src/sample.rs` consumes these types: voices copy their slice region at
//! note-on and own their DSP state, so swapping a slice map or shaping
//! settings never invalidates a sounding voice and nothing here touches the
//! filesystem. Absent blocks preserve the legacy playback path bit-for-bit.

pub mod command;
pub mod shaping;
pub mod slices;

pub use command::{apply_sample_command, parse_sample_command, SampleCommand};
pub use shaping::{
    CompiledShaping, SampleEnvelope, SampleFilterMode, SampleShaping, VoiceShaping, SHAPING_VERSION,
};
pub use slices::{
    detect_slices, equal_slices, SliceDef, SliceSelect, SliceSettings, SliceTable, MAX_SLICES,
    SLICES_VERSION,
};
