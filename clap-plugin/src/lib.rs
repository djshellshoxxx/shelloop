//! SHELLOOP as a CLAP instrument plugin.
//!
//! The plugin wraps the core crate's polyphonic [`shelloop::RealtimeSynth`]
//! and plays the bundled example bassline with a host-synced step sequencer.
//! The raw CLAP ABI comes from `clap-sys`; all `unsafe` lives in [`ffi`].
//!
//! Layout:
//! - [`params`]: stable parameter table, text conversion, patch building
//! - [`sequencer`]: beat-based sequencer following the host transport
//! - [`processor`]: audio-thread state (synth, sequencer, note tracking)
//! - [`state`]: versioned JSON state blob
//! - [`ffi`]: entry point, factory, plugin vtable and extensions

pub mod ffi;
pub mod params;
pub mod processor;
pub mod sequencer;
pub mod state;

pub use ffi::clap_entry;

/// CLAP plugin id.
pub const PLUGIN_ID: &str = "com.circuitdriftlabs.shelloop";

/// The bundled pattern played by the built-in sequencer.
pub const BUNDLED_PATTERN_JSON: &str = include_str!("../../patterns/example-bassline.json");
