//! Musical helpers (scales, chords, arpeggios, Euclidean rhythms, tap
//! tempo, humanize, step ratios and pattern edit transforms).
//!
//! Everything here is control-thread or pure data: no audio device, no
//! filesystem and no allocation on the real-time path. The controller turns
//! these transforms into ordinary pattern edits, so they stay undoable and
//! use the existing quantised revision path.
//!
//! See `docs/MODULES.md` for the module index and invariants.

pub mod arp;
pub mod chord;
pub mod command;
pub mod edit;
pub mod euclid;
pub mod humanize;
pub mod ratio;
pub mod scale;
pub mod tap;

pub use arp::{ArpMode, Arpeggio, MAX_ARP_OCTAVES};
pub use chord::{chord_notes, ChordQuality, MAX_CHORD_NOTES};
pub use command::{parse_music_command, MusicCommand};
pub use edit::{apply_arp, apply_chord, apply_euclid, quantize_notes, transpose_pattern};
pub use euclid::{euclidean, MAX_EUCLID_STEPS};
pub use humanize::{Humanize, MAX_HUMANIZE_TIMING_FRAMES, MAX_HUMANIZE_VELOCITY};
pub use ratio::{parse_ratio, StepRatio, MAX_RATIO_CYCLE};
pub use scale::{parse_note, parse_pitch_class, Scale, ScaleKind};
pub use tap::TapTempo;
