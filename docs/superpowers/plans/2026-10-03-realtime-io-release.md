# Real-time I/O and Beta Release Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Turn the verified Shelloop musical core into a runnable terminal beta with guarded audio/MIDI backends, then QA and publish Windows/Linux v0.01-beta artifacts.

**Architecture:** Keep hardware-facing CPAL and midir code behind optional features and small adapters. Move all deterministic behavior (device-selection policy, sample conversion, MIDI port selection/reconnect decisions, keyboard mapping) into pure functions covered by tests so CI can prove behavior without physical hardware. The binary owns lifecycle and error reporting; the audio callback remains allocation-free once started and communicates through bounded channels/state.

**Tech Stack:** Rust 2021, CPAL 0.16, midir 0.10, crossterm 0.29, ratatui 0.29, GitHub Actions.

**Spec:** Existing approved Shelloop architecture represented by `docs/superpowers/plans/2026-10-03-musical-engine.md` and PR #1 scope.

## Global Constraints

- Windows and Linux are primary beta targets.
- No blocking, filesystem access, terminal rendering, or unbounded queue growth in the audio callback.
- Sample-frame timing remains authoritative.
- Optional backends must compile in CI on both Windows and Linux.
- Hardware absence must produce actionable errors, not panics.
- Release tag/version requested by the user: `v0.01-beta`.

## Review Focus

- No audio output device available: return a clear error and keep process state sane.
- Unsupported sample formats/channel counts: convert safely or reject explicitly.
- MIDI device disconnect/reconnect: do not panic or leave stuck notes.
- Terminal input exit/panic paths: always request all-notes-off before shutdown.
- Release artifacts: binaries must come from the exact verified release commit and include SHA-256 checksums.

---

### Task 1: Feature-build CI guardrail

**Files:**
- Modify: `.github/workflows/ci.yml`

**Interfaces:**
- Produces: Linux and Windows `cargo check`/Clippy coverage for all optional features.

- [x] Add Linux system audio headers and all-feature check/clippy job.
- [x] Add equivalent Windows all-feature check/clippy job.
- [ ] Verify the workflow passes on both operating systems.

### Task 2: Audio backend adapter

**Files:**
- Create: `src/audio.rs`
- Modify: `src/lib.rs`
- Test: `tests/audio_contracts.rs`

**Interfaces:**
- Produces: pure sample-format/channel helpers plus `#[cfg(feature = "realtime-audio")]` device enumeration/default-device stream setup.

- [ ] Write failing tests for stereo frame writing, mono duplication, clipping/non-finite protection, and deterministic device-name selection.
- [ ] Run tests and verify RED.
- [ ] Implement minimal pure audio helpers.
- [ ] Add CPAL adapter that enumerates output devices, selects requested/default device, builds an output stream, and reports stream errors without panicking.
- [ ] Run default and all-feature test/check/clippy suites.

### Task 3: MIDI I/O adapter

**Files:**
- Create: `src/midi_io.rs`
- Modify: `src/lib.rs`
- Test: `tests/midi_io_contracts.rs`

**Interfaces:**
- Consumes: `decode_message(&[u8]) -> Result<MidiEvent, String>`.
- Produces: deterministic port selection and feature-gated midir connection wrapper using a bounded event sender.

- [ ] Write failing tests for exact-name/index/default port selection and reconnect-policy decisions.
- [ ] Run tests and verify RED.
- [ ] Implement pure selection/reconnect policy.
- [ ] Add midir enumeration/connection wrapper; malformed MIDI is dropped with diagnostics rather than panicking.
- [ ] Run default and all-feature test/check/clippy suites.

### Task 4: Terminal performance controls and runnable binary

**Files:**
- Create: `src/keyboard.rs`
- Create: `src/app.rs`
- Modify: `src/main.rs`, `src/lib.rs`
- Test: `tests/keyboard_contracts.rs`, `tests/app_contracts.rs`

**Interfaces:**
- Consumes: transport, voice, MIDI, audio adapters.
- Produces: keyboard note mapping, panic/quit controls, CLI startup options, terminal event loop.

- [ ] Write failing tests for keyboard note map, octave bounds, panic/quit commands, and startup-option parsing.
- [ ] Run tests and verify RED.
- [ ] Implement pure keyboard/options logic.
- [ ] Wire terminal event loop and backend lifecycle behind features.
- [ ] Ensure quit/error paths request panic/all-notes-off.
- [ ] Run full default and all-feature verification.

### Task 5: QA hardening

**Files:**
- Modify only files implicated by failures.
- Create: `docs/QA-v0.01-beta.md`

**Interfaces:**
- Produces: reproducible QA record and any RED→GREEN fixes.

- [ ] Run fmt, full tests, strict clippy, release builds, and Windows/Linux feature builds.
- [ ] Exercise no-device/error paths through tests and CLI smoke checks.
- [ ] Audit panic/unwrap usage in runtime paths and fix important findings test-first.
- [ ] Record limitations requiring physical hardware validation.

### Task 6: v0.01-beta release pipeline and artifacts

**Files:**
- Modify: `Cargo.toml`
- Create: `.github/workflows/release.yml`
- Create/Modify: `README.md`

**Interfaces:**
- Produces: Windows `.exe`, Linux executable/archive, SHA-256 checksums, GitHub prerelease `v0.01-beta`.

- [ ] Set package version corresponding to the requested 0.01 beta.
- [ ] Add tag-triggered Windows/Linux release builds with all runtime features enabled.
- [ ] Package binaries with README/license and generate SHA-256 checksums.
- [ ] Verify release workflow from the exact green commit.
- [ ] Publish `v0.01-beta` as a GitHub prerelease and attach artifacts.
