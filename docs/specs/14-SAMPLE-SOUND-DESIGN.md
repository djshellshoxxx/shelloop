# Spec 14 — Sample shaping: transient, filter, envelope and slice playback

Status: implemented (see the implementation notes at the end of this document). Scope: sample tracks, resampling path and their saved projects.

## Goal
Make one-shot/looped samples more expressive for drums, breaks, bass fragments and texture design while retaining deterministic sequencing.

## Features
- Per-sample low-pass/high-pass multimode filter and optional drive, both bypassable and smoothed.
- Four-stage per-voice amplitude envelope with an explicit one-shot mode preserving legacy behavior.
- Non-destructive transient shaping (attack and sustain gain) with strictly bounded lookahead: initial release may use zero-lookahead envelope followers; report latency if future modes add lookahead.
- Slice region definitions in sample-frame coordinates, selected by step lock, MIDI note mapping or deterministic seeded random choice. Preserve reverse playback and root-note transpose semantics.
- Optional tempo-relative playback pitch mode only if its pitch-changing behavior is clearly labeled. High-quality pitch-preserving time stretch is deliberately out of initial scope.

## Architecture
Store immutable validated slice metadata in the sample asset or prepared track configuration. Separate disk decoding and slice detection (control/background thread) from audio rendering. Existing `SampleVoice` should receive bounded per-voice DSP state and sample positions but must not own file handles. Commit new slice maps atomically at a musical boundary and keep the old shared sample memory alive until all existing voices are retired. Never allocate, lock or perform file IO in the callback. Use a fixed maximum of 128 slices per sample, 64 voices per track and existing sample-bank memory budget. Validate boundaries and reject empty/overlapping slices unless overlap semantics are explicitly supported.

## Data and compatibility
Introduce optional versioned `sample_shaping` and `slices` blocks; absent fields preserve current gain, fade, playback, reverse, pitch, loop and gate behavior. Convert normalized regions to frame indices on the control thread. Validate malformed and non-finite parameters before preparing voices.

## Interface and automation
Provide TUI commands `slice list/add/delete/assign` and `sample filter/envelope/transient` with typed validation and undo transactions. Add step-lock IDs for slice select and filter cutoff. Ensure CLAP sample-track parity is assessed before claiming plugin support; where the plugin only contains the synth, keep this feature standalone until a separate shared sample engine is integrated.

## Tests
Legacy playback parity; 44.1/48/96 kHz envelopes; all slices bounded in forward/reverse and loop modes; click-boundary fades; note-stealing and panic; step locks; preset migration; randomized input fuzz/property tests; finite audio; artifact rendering and peak/CPU benchmark. Include WAV fixtures with transients and silence, then hardware listening tests.

## Delivery
1. Pure slicing and validation library with tests.
2. Prepared voice DSP and integration tests.
3. Control/TUI editing and examples.
4. Optional automated slice markers offline.
5. Host plugin support only after parity and regression gates.

## Implementation notes

Implemented in `src/sample_shaping/` (`slices`, `shaping`, `command`) and integrated into `src/sample.rs`. Tests: `tests/sample_shaping.rs`, `tests/example_projects.rs`, `tests/realtime_allocations.rs`.

* **Order**: transient shaper → drive → filter → envelope/gain. Neutral stages are skipped, so default settings are bit-identical to the legacy path.
* **Slices**: normalised positions in project files; a voice copies its frame region at note-on, so replacing the table never disturbs sounding voices. A new table applies to notes started after the message is processed; it is **not** deferred to a musical boundary.
* **Selection**: `note` (silent outside the mapped range), seeded `random` (rewinds on panic) or the `sample.slice` step lock; a lock beyond the slice count plays the whole region.
* **Envelope**: the ADSR replaces the legacy attack/release fades only when configured. In `one_shot` mode note-off is still ignored.
* **Cutoff lock**: `sample.filter_cutoff` also switches the filter on (low-pass) for that step when the configured mode is `off`.
* **Detection**: `slice detect [count] [sensitivity]` (added 2026-10-21, spec 14 delivery step 4) decodes the selected track's WAV on the control thread with `decode_wav_file` under the sample memory budget, runs `detect_slices` and replaces the slice list as one undoable `sample undo` transaction. Defaults: 16 slices, sensitivity 0.5. The audio thread only receives the resulting `SliceTable`.
* **Open deviation**: the Acceptance text above asks that new slice maps commit at a musical boundary. The current implementation applies them immediately to notes started after the message (see Slices above). Quantised commit is tracked in `docs/OPEN-ITEMS.md` and is not claimed as complete.
* **Out of scope as specified**: tempo-relative pitch mode and time-stretch. The CLAP/VST3 plugin has no sample engine, so this feature is standalone only.
