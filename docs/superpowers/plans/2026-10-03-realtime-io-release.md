# Real-time I/O and Beta Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the verified Shelloop musical core into a runnable terminal beta with guarded audio/MIDI backends, then QA and publish Windows/Linux v0.01-beta artifacts.

**Architecture:** Keep hardware-facing CPAL and midir code behind optional features and small adapters. Move deterministic behavior into testable functions and keep the audio callback bounded. Pattern files are loaded and validated before audio startup; live pattern scheduling uses sample-frame timing and preallocated state in the callback. The binary owns lifecycle and error reporting, while terminal/MIDI/sequencer controls cross fixed-capacity channels.

**Tech Stack:** Rust 2021, CPAL 0.16, midir 0.10, crossterm 0.29, ratatui 0.29, GitHub Actions.

**Spec:** Existing approved Shelloop architecture represented by `docs/superpowers/plans/2026-10-03-musical-engine.md` and PR #1 scope.

## Global Constraints

- Windows and Linux are primary beta targets.
- No blocking filesystem access, terminal rendering, or unbounded queue growth in the audio callback.
- Sample-frame timing remains authoritative.
- Optional backends must compile in CI on both Windows and Linux.
- Hardware absence must produce actionable errors, not panics.
- Release tag/version requested by the user: `v0.01-beta`.

## Review Focus

- No audio output device available: return a clear error and keep process state sane.
- Unsupported sample formats/channel counts: convert safely or reject explicitly.
- MIDI device disconnect/reconnect: do not panic or leave stuck notes.
- Terminal input exit/panic paths: always request all-notes-off before shutdown.
- Sequencer pause/restart must not cut live keyboard/MIDI voices.
- Release artifacts: binaries must come from the exact verified release commit and include SHA-256 checksums.

---

### Task 1: Feature-build CI guardrail

**Files:**
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: Linux and Windows check/test/Clippy/release-build coverage for all optional features.

- [x] Add Linux system audio headers and all-feature check/clippy job.
- [x] Add equivalent Windows all-feature check/clippy job.
- [x] Verify the workflow passes on both operating systems. Reverified after live sequencer integration on CI run #106 / `37201995320` at implementation head `bf25d3b7f503a690094afcb9b411291c23c93af5`.

### Task 2: Audio backend adapter

**Files:**
- Create: `src/audio.rs`
- Modify: `src/lib.rs`
- Test: `tests/audio_contracts.rs`

**Interfaces:**
- Produces: pure sample-format/channel helpers plus `#[cfg(feature = "realtime-audio")]` device enumeration/default-device stream setup.

- [x] Write failing tests for stereo frame writing, mono duplication, clipping/non-finite protection, deterministic device-name selection, and renderer sample-rate construction.
- [x] Run tests and verify missing behavior before implementation.
- [x] Implement minimal pure audio helpers.
- [x] Add CPAL adapter that enumerates output devices, selects requested/default device, builds an output stream, and reports stream errors without panicking.
- [x] Construct the real-time renderer after CPAL reports the actual selected-device sample rate.
- [x] Run default and all-feature test/check/clippy suites.

### Task 3: MIDI I/O adapter

**Files:**
- Create: `src/midi_io.rs`
- Modify: `src/lib.rs`
- Test: `tests/midi_io_contracts.rs`, `tests/live_runtime_contracts.rs`

**Interfaces:**
- Consumes: `decode_message(&[u8]) -> Result<MidiEvent, String>`.
- Produces: deterministic port selection, live MIDI-to-engine command mapping, and feature-gated midir connection wrapper using bounded event delivery.

- [x] Write failing tests for exact-name/index/default port selection and reconnect-policy decisions.
- [x] Run tests and verify missing behavior.
- [x] Implement pure selection/reconnect policy.
- [x] Add midir enumeration/connection wrapper; malformed MIDI is dropped/countable rather than panicking.
- [x] Add tested routing for note on/off, sustain and all-notes-off/panic commands.
- [x] Add runtime re-enumeration/reconnect with panic on selected-device disappearance.
- [x] Run default and all-feature test/check/clippy suites.

### Task 4: Terminal performance controls and runnable binary

**Files:**
- Create: `src/keyboard.rs`
- Create: `src/app.rs`
- Create: `src/runtime.rs`
- Modify: `src/main.rs`, `src/lib.rs`
- Test: `tests/keyboard_contracts.rs`, `tests/app_contracts.rs`, `tests/cli_contracts.rs`

**Interfaces:**
- Consumes: voice, MIDI and audio adapters.
- Produces: keyboard note mapping, panic/quit controls, CLI startup options, live terminal event loop and real-time playable synth.

- [x] Write failing tests for keyboard note map, octave bounds, panic/quit commands, startup-option parsing, CLI help/error behavior and feature-disabled execution.
- [x] Run tests and verify missing behavior before implementation.
- [x] Implement pure keyboard/options logic.
- [x] Wire terminal event loop and backend lifecycle behind features.
- [x] Wire keyboard and MIDI note events into `RealtimeSynth` through a fixed-capacity command channel.
- [x] Ensure quit/error paths request panic/all-notes-off.
- [x] Request crossterm key-release event reporting where supported and provide a bounded timed fallback otherwise.
- [x] Run full default and all-feature verification.

### Task 5: QA hardening

**Files:**
- Modify only files implicated by failures.
- Create: `docs/QA-v0.01-beta.md`

**Interfaces:**
- Produces: reproducible QA record and any RED→GREEN fixes.

- [x] Run fmt, full tests, strict clippy, release builds, and Windows/Linux feature builds. Reverified on CI run #106.
- [x] Exercise deterministic error/validation paths through tests and CLI smoke contracts; hardware-absence behavior remains part of physical host validation.
- [x] Audit panic/unwrap usage in runtime paths and fix important findings test-first. No explicit production `panic!` was found; the allocator `expect` is guarded by its nonzero-polyphony constructor invariant.
- [x] Record limitations requiring physical hardware validation in `docs/QA-v0.01-beta.md`.

### Task 6: Live sequencer playback integration

**Files:**
- Create: `src/sequencer.rs`, `tests/live_sequencer_contracts.rs`, `patterns/example-bassline.json`
- Modify: `src/pattern.rs`, `src/runtime.rs`, `src/app.rs`, `src/main.rs`, `src/lib.rs`, `README.md`

**Interfaces:**
- Consumes: deterministic `PatternScheduler`, `RealtimeSynth`, CPAL callback sample rate, terminal performance controls.
- Produces: validated JSON pattern loading, live sample-frame playback, bounded sequencer transport controls and separate performance/sequencer synth paths.

- [x] Define failing live sequencer contracts before implementation.
- [x] Add reusable/preallocated pattern scheduling buffers.
- [x] Add `LiveSequencer` with gate-timed note-offs, pause/resume and restart semantics.
- [x] Add CLI pattern/BPM/grid options and validate JSON before audio startup.
- [x] Connect sequencer playback to the real-time CPAL callback.
- [x] Keep sequencer voices separate from live keyboard/MIDI voices.
- [x] Add Space play/pause, Backspace restart and sequencer panic routing.
- [x] Add an example pattern and usage documentation.
- [x] Verify default and Windows/Linux all-feature check/test/strict-Clippy/release-build jobs on CI run #106.

### Task 7: v0.01-beta release pipeline and artifacts

**Files:**
- Modify: `Cargo.toml`
- Create: `.github/workflows/release.yml`
- Create/Modify: `README.md`

**Interfaces:**
- Produces: Windows `.exe`, Linux executable/archive, SHA-256 checksums, GitHub prerelease `v0.01-beta`.

- [x] Set package version corresponding to the requested 0.01 beta (`0.1.0-beta.1`).
- [x] Add tag-triggered Windows/Linux release builds with all runtime features enabled.
- [x] Package binaries with README/license and generate SHA-256 checksums.
- [ ] Complete representative physical Windows/Linux audio and MIDI validation from `docs/QA-v0.01-beta.md`.
- [ ] Verify the actual tag-triggered release packaging workflow from the exact final release commit.
- [ ] Publish `v0.01-beta` as a GitHub prerelease and attach artifacts.

## Current Gate

The selected beta code scope, including live sequencer pattern playback, is implemented and passes the automated cross-platform gate recorded in `docs/QA-v0.01-beta.md`.

Do not create the release tag yet. The remaining release gate requires representative physical Windows/Linux audio and MIDI validation, including real output-device behavior, MIDI unplug/reconnect, latency/xrun/sound-quality checks, and simultaneous live-performance plus sequencer playback. After physical validation, verify the tag-triggered packaging workflow from the exact release commit and publish the prerelease.

Broader v1 implementation can proceed independently on quantized live pattern/scene replacement, terminal command/display work, mouse XY control, drum/sample playback, effects and recording.
