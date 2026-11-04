# Shelloop audit — 2026-10-21 (branch claude/shelloop-autonomous)

Scope: changes made on this branch (slice detect, bar-boundary slice commits, Euclidean editor command, bitcrusher effect), plus the audio-thread, control/audio handoff and persistence paths they touch. Method: code reading, targeted probes and regression tests. No external host or hardware testing was done.

## Findings

| ID | Severity | Status | Finding | Evidence | Fix / verification |
|---|---|---|---|---|---|
| F-01 | Medium | Fixed | A slice map queued just before a restart waited for a frame on the restarted clock, up to one bar of stale behaviour. | `tests/sample_shaping.rs::queued_slice_map_commits_at_once_when_the_clock_restarts` failed before the fix (left 4, right 2) | `restart_all` now commits pending maps (commit f6dfc20). Test passes. |
| F-02 | Low | Open (decision) | `parse_command` accepts `tempo`/`bpm` and returns `Command::Tempo`, but nothing executes `Command`. Only parsing tests reference it. The README does not document it. | `src/command.rs`; `tests/runtime_contracts.rs:96`; no handler found by grep | Not changed: public API. Remove, or wire to a runtime tempo change (which would also need LFO and effect tempo updates). |
| F-03 | Info | Documented | Paused tracks do not advance their clock, so pattern revisions still wait until play resumes. Slice maps now commit at once when stopped. | `src/sequencer.rs` `fill_commands_with_locks` returns early when not playing | Behaviour kept for patterns, documented in spec 14. |
| F-04 | Info | By design | Pattern invariant profiles are validated for shape only. Manual and Euclidean edits can change steps that an invariant names as anchors. | `src/pattern.rs` validate calls `profile.validate(len)` only | Documented in spec 16. Variation (spec 12) still enforces its own invariants. |
| F-05 | Info | Not a defect | A pattern-boundary edit queued at frame 0 applies at frame 1 (same for `rotate`). An early test expectation of revision 2 was wrong. | Probe: `euclid 4`, `rotate right 1`, `step 2 toggle` all transition at frame 1 | Test corrected. |
| F-06 | Low | Known gap | Beat-repeat grid counts frames from its own reset, not the transport. | Spec 15 implementation notes | Tracked as OI-05. |
| F-07 | Low | Open, unmeasured | `slice detect` decodes the whole file on the control thread (up to the 512 MiB sample budget). The header check runs before allocation, but a large valid file would still block the control thread for an unmeasured time. | `src/controller.rs` SliceDetect arm; `decode_wav_file` header check | Not measured. Candidate: a smaller limit for detection. |
| F-08 | Info | Checked | Audio-thread additions (`pending_slices` check, bitcrusher `process`) use no allocation, locks, I/O or logging. The `realtime_allocations` test covers the bitcrusher. | Code reading; `tests/realtime_allocations.rs` passes | None needed. |
| F-09 | Info | Checked | Project sample paths are resolved relative to the project folder and read with the same WAV decoder used at load time. No network access. Decode refuses oversized headers before allocating. | `src/sample.rs` `decode_wav_reader` | None needed. |

## Verification status
* `cargo test` (default features): 450 tests passing on Linux, after the final change.
* Full-feature tests and strict clippy: see the build report in PR #13; re-run recorded there.
* Not verified: DAW hosts, audio quality, latency, Windows/macOS behaviour (CI only), CPU cost of the new code (not measured).

## Build status (Linux, local, 2026-10-21, head after the restart fix)

| Build | Command | Result |
|---|---|---|
| Default tests | `cargo test` | 450 passed, 0 failed |
| Full-feature tests | `cargo test --workspace --all-targets --features realtime-audio,midi,terminal-ui` | 489 passed, 0 failed |
| Strict clippy, default | `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| Strict clippy, features | same with `--features realtime-audio,midi,terminal-ui` | pass |
| Format | `cargo fmt --all -- --check` | pass |
| Release runtime | `cargo build --release --features realtime-audio,midi,terminal-ui` | pass |
| CLAP plugin | `cargo build --release -p shelloop-clap` | pass |
| VST3 | `python3 scripts/build_vst3.py` | pass with GCC 13.3; factory verified (`verify_vst3.py`). Fails on CI GCC 15.2 in clap-wrapper, see below |
| Linux packages | packaged as in `release.yml`, `scripts/verify_release.py` | pass; checksums verified from `dist/` (gitignored) |

Windows and macOS were not built locally. The repo's CI covers Windows (features, VST3, release). The project does not define a macOS build job in CI, so no macOS build exists to report.

## CI finding (pre-existing)

`VST3 (ubuntu-latest)` fails on `main` at 0c319da and on this branch. Root cause in the job log: clap-wrapper v0.9.1 (fetched by CMake) fails to compile `sha1.cpp` under GCC 15.2 with `-Werror=stringop-overflow`. This branch does not change the VST3 build. Reported on PR #13. Options are recorded in that comment.
