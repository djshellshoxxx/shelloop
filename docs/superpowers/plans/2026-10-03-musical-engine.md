# Shelloop Musical Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the deterministic, bounded musical engine layer that sits between Shelloop's control inputs and the future real-time audio callback.

**Architecture:** Keep musical state independent of CPAL, terminal, MIDI-device and disk-I/O backends. Patterns compile into sample-frame note events using deterministic seeded probability, swing, microtiming and ratchets; a bounded voice allocator consumes note lifecycle events; scene changes are represented as quantized immutable replacements so later audio/control threads can exchange them without embedding UI or device logic in the engine.

**Tech Stack:** Rust 2021, serde, rand SmallRng, existing Shelloop scheduler/synth/transport modules, GitHub Actions for rustfmt/tests/clippy.

**Spec:** Recovered Shelloop engineering brief `Pasted text(20261003-194520).txt`, especially live sequencer requirements (per-step note/velocity/duration/probability/accent/microtiming, ratchets, unequal lengths, deterministic seeds), playable synth voice stealing/sustain/panic, and sample-frame scheduling acceptance tests.

## Global Constraints

- Windows and Linux remain primary targets.
- The audio callback must eventually have bounded work, bounded resources and no blocking I/O, terminal output, mutex contention or unbounded allocation.
- The sample-frame transport clock remains scheduling authority.
- Event placement must use exact offsets within an audio block.
- Probability must be deterministic for a stored seed.
- Pattern lengths are independent per track.
- Tempo changes, loop boundaries and event ordering must not duplicate or drop events.
- Voice allocation must have a fixed maximum polyphony and deterministic stealing.
- Panic must immediately make all voices inactive.
- This plan does not claim physical real-time readiness; CPAL/device/hardware validation is a later plan.

## Review Focus

- Loop-boundary events: an event exactly at the next loop/block boundary appears once, never twice.
- Negative microtiming: events shifted before frame zero clamp/drop according to explicit scheduling rules rather than wrapping to huge unsigned values.
- Deterministic probability: identical project seed/pattern/loop produces identical trigger decisions across runs.
- Ratchet overload: ratchet count is validated and scheduling remains bounded.
- Voice exhaustion: stealing is deterministic and a stolen voice cannot later emit a stale note-off against its replacement.

---

### Task 1: Validated Pattern Model and Deterministic Step Expansion

**Files:**
- Create: `src/pattern.rs`
- Modify: `src/lib.rs`
- Test: `tests/pattern_engine.rs`

**Interfaces:**
- Consumes: `Scheduler::frames_per_step()` semantics and sample-frame clock units.
- Produces: `PatternStep`, `Pattern`, `PatternEvent`, `PatternScheduler::schedule_block(...) -> Vec<PatternEvent>`.

- [ ] **Step 1: Write failing tests** for validation, independent loop length, deterministic probability, swing, signed microtiming, ratchets and loop-boundary de-duplication.
- [ ] **Step 2: Run `cargo test --test pattern_engine` and verify the new tests fail for missing pattern behavior.**
- [ ] **Step 3: Implement `PatternStep`, `Pattern`, and `PatternScheduler`** with bounded ranges: pattern length 1..=256 steps, velocity/probability 0..=1, ratchets 1..=8, gate 0..=1, swing 0..=0.75 step, signed microtiming expressed in frames. Use a stable per-event seed derived from project seed + pattern seed + loop index + step index so decisions do not depend on call/block partitioning.
- [ ] **Step 4: Run `cargo test --test pattern_engine` and verify all pattern tests pass.**
- [ ] **Step 5: Run the full suite and strict clippy; commit.**

### Task 2: Bounded Voice Allocator, Sustain and Panic

**Files:**
- Create: `src/voice.rs`
- Modify: `src/lib.rs`
- Test: `tests/voice_lifecycle.rs`

**Interfaces:**
- Consumes: note/channel/velocity semantics already used by `MidiEvent` and the future pattern events.
- Produces: `VoiceId`, `VoiceState`, `VoiceAllocator::note_on`, `note_off`, `set_sustain`, `release_sustain`, `panic`, and deterministic oldest-voice stealing.

- [ ] **Step 1: Write failing tests** for bounded polyphony, note-on/off lifecycle, repeated notes, sustain-deferred release, deterministic stealing and panic.
- [ ] **Step 2: Run `cargo test --test voice_lifecycle` and verify failures correspond to missing lifecycle behavior.**
- [ ] **Step 3: Implement a pre-sized logical voice pool** with monotonically increasing generation/order counters; never allow more than configured maximum active voices. Note-off targets the oldest matching active note that has not already been released; sustain defers release; stealing chooses the oldest releasable voice first, otherwise oldest active voice.
- [ ] **Step 4: Run the focused and full suites; strict clippy must pass.**
- [ ] **Step 5: Commit.**

### Task 3: Quantized Scene/Pattern Replacement Primitives

**Files:**
- Create: `src/quantize.rs`
- Modify: `src/lib.rs`
- Test: `tests/quantized_changes.rs`

**Interfaces:**
- Consumes: absolute sample-frame positions, BPM, sample rate and steps-per-beat.
- Produces: `QuantizeBoundary::{Immediate, Step, Beat, Bar, Pattern}`, `QuantizedChange<T>`, and `next_boundary_frame(...)`.

- [ ] **Step 1: Write failing tests** for immediate/step/beat/bar boundaries, exact-boundary idempotence, odd-meter bars, pattern-length boundaries and tempo validation.
- [ ] **Step 2: Run `cargo test --test quantized_changes` and verify the new behavior is absent.**
- [ ] **Step 3: Implement pure boundary math** with no wall-clock state. An edit requested exactly on a boundary applies there; otherwise it applies at the first later boundary. Pattern boundaries use the target track's own pattern length.
- [ ] **Step 4: Run the focused suite, then all tests and strict clippy.**
- [ ] **Step 5: Commit.**

## Completion Gate

This plan is complete only when `cargo fmt --all -- --check`, `cargo test --all-targets`, and `cargo clippy --all-targets -- -D warnings` all pass on the current branch head. The next plan may then wire this engine into CPAL/midir/terminal backends without redefining musical timing or voice semantics.
