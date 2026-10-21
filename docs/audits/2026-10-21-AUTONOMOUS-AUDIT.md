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
