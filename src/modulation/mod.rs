//! Spec 13 — modulation matrix and performance macros.
//!
//! * [`model`] — serialisable, validated patch data (LFOs, macros, routes).
//! * [`compile`] — control-thread compilation into a `Copy`, allocation-free
//!   form the audio thread can apply.
//! * [`runtime`] — per-voice LFO state, control-rate evaluation and
//!   interpolated offsets.
//!
//! Evaluation order (documented contract): base patch value → active step
//! lock → summed modulation → final clamp. Modulation offsets are evaluated
//! every [`runtime::MOD_CONTROL_INTERVAL`] samples and linearly interpolated
//! between update points, which is also the modulation smoothing stage.

pub mod command;
pub mod compile;
pub mod model;
pub mod runtime;

pub use command::{apply_mod_command, describe_routes, parse_mod_command, ModCommand};
pub use compile::{CompiledModulation, CompiledRoute};
pub use model::{
    parse_mod_dest, parse_mod_source, LfoConfig, LfoWave, ModDest, ModRoute, ModSource,
    ModulationConfig, SyncDivision, MAX_LFOS, MAX_MACROS, MAX_ROUTES, MODULATION_VERSION,
};
pub use runtime::{ModInputs, ModOffsets, ModRuntime, MOD_CONTROL_INTERVAL};
