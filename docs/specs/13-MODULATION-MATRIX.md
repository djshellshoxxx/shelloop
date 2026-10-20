# Spec 13 — Modulation matrix and performance macros

Status: implemented (see the implementation notes at the end of this document). Target: standalone engine plus CLAP/VST3 where applicable.

## Goal
Add expressive, tempo-aware modulation without breaking the v1 projects or allocating on the real-time thread. Support sound design for evolving pads, bass movement, and rhythmic effects.

## User-visible behavior
Each synth patch supports up to 8 modulation routings and 2 macro controls. Sources: two LFOs, amplitude envelope, filter envelope, velocity, key tracking, mod wheel, and two macros. Destinations: oscillator tuning (bounded), pulse width, filter cutoff/resonance, amplitude, pan, and effect wet mix where a destination is present. LFO waves: sine, triangle, saw, square and deterministic sample-and-hold. LFO rate supports Hz or tempo-relative divisions. Each route has a bipolar amount, enabled flag, and optional smoothing duration.

## Data contract
Introduce a versioned `modulation` object with `lfos`, `macros`, and `routes`. Strictly validate finite numbers, valid destination/source IDs, capacity limits and ranges. Omitted modulation defaults to no modulation, preserving existing patches and project serialization. Never silently retarget a removed parameter; preserve a disabled/orphaned route where possible.

## Engine design
Introduce `src/modulation/` with separate `model.rs`, `compile.rs`, `runtime.rs`, and `tests` modules. Compile strings/IDs to typed enumerations on the control thread. The audio thread owns fixed arrays of prepared routes and LFO state; no heap allocation, locks, blocking or dynamic graph traversal. Evaluate per block for control-rate destinations and per sample for audio-rate-sensitive pitch if needed; interpolate modulation between update points. Explicitly define interaction order: base patch value → active step lock → summed modulation → final clamp → parameter smoothing. Ensure sample-rate changes reconstruct coefficients without changing wall-clock modulation rate. Reset or preserve LFO phase on restart according to documented per-LFO setting.

## Control and plugin wiring
Expose stable command IDs for LFO rates/waves, route source/destination/depth, and macro values. Add TUI inspector/editor and clear visual feedback for effective destination values. Publish a conservative stable subset as host-automatable CLAP parameters; synchronize state-save/load and wrapper VST3 behavior. Never modify parameters or publish host change notifications from the audio callback in violation of CLAP threading rules.

## Test gates
1. Zero modulation is bit-identical to legacy rendering at the same configuration.
2. LFO frequencies and tempo subdivisions match across 44.1/48/96 kHz and tempo changes.
3. Deterministic random seed gives identical sample-and-hold sequence.
4. Multiple routes sum in defined order; all edge inputs stay finite and within bounds.
5. Step locks override base without changing stored base values; modulation clamps after locks.
6. No allocations in a warmed audio render; callback meets timing budget.
7. Serialization round-trips and legacy projects load unchanged.
8. CLAP state and automation smoke tests; VST3 wrapper validation; Windows/Linux CI.

## Delivery
Implement engine and unit tests first, then control commands/TUI, then CLAP state/automation, then examples and docs. Preserve stable IDs and introduce migration tests before integration.

## Implementation notes

Implemented in `src/modulation/` (`model`, `compile`, `runtime`, plus `command` for the `mod` text commands). Tests live in `tests/modulation.rs` and `tests/realtime_allocations.rs` rather than an in-module `tests` directory.

* **Evaluation**: base value → active step lock → summed modulation → final clamp. Offsets update every 16 samples and are linearly interpolated, which doubles as the smoothing stage; routes may add a one-pole `smoothing_ms`. LFOs are evaluated at the control rate (≈0.33 ms at 48 kHz).
* **Destinations**: tuning, pulse width, filter cutoff/resonance and amplitude act per voice. `pan` and `effect_wet` are track-level: the engine averages the sounding voices' offsets and the track strip adds them to its pan and send levels (one sample late).
* **Orphans**: unknown source/destination names fail validation on load rather than being preserved as disabled routes; nothing is ever silently retargeted.
* **Tempo**: tempo-synced LFOs use the project BPM given at engine construction.
* **Plugin**: CLAP parameters 15–22 expose LFO 1, five fixed depths and a brightness macro; the VST3 wrapper inherits them. A full 8-route editor is a standalone feature.
* **Not verified**: DAW automation and VST3 validator runs on real hosts (CI covers factory loading only).
