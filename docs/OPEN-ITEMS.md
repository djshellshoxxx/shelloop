# Open items and requirement status (2026-10-21)

Traceability register for work that is not complete. Each row names the requirement source, its status and the evidence needed to close it. "Automated" means it can be checked by `cargo test` on Linux; "External" needs real hardware, a real DAW or CI on another OS.

| ID | Source | Item | Status | Closure evidence |
|---|---|---|---|---|
| OI-01 | Spec 14 | Quantised commit of slice maps | Done 2026-10-21 (bar boundary, see spec 14) | `tests/sample_shaping.rs::slice_maps_commit_*`, restart test |
| OI-02 | Spec 14 | `slice detect` command | Done 2026-10-21 | `tests/sample_shaping.rs::slice_detect_*`, `controller_slice_detect_*` |
| OI-03 | Spec 14 | CLAP/VST3 sample-track parity | Open, out of scope as specified | Requires a shared sample engine in the plugin |
| OI-04 | Spec 14 | Tempo-relative pitch / time-stretch | Out of scope as specified | Not planned |
| OI-05 | Spec 15 | Beat-repeat grid aligned to transport | Done 2026-10-22 | Absolute-frame context reaches track, send and master racks; `beat_repeat_uses_transport_grid_when_inserted_after_playback_started` covers late insertion, pause and seek. Legacy process APIs remain compatible. |
| OI-06 | Spec 03 | Preset directory management, saving live control snapshots | Done 2026-10-22 | `PresetStore` versioned atomic files; controller preset/load/save snapshot and queue-rejection tests |
| OI-07 | Spec 04 | Sample load mode | Resolved: fail-before-activation | Spec permits refusal or unavailable tracks. Every asset error is reported together; active bank/session remains intact. No silent partial playback. Regression in `wav_sample_playback`. |
| OI-08 | Spec 13 | Full 8-route TUI editor and conservative CLAP subset | Done / supported scope | All eight routes edit through TUI `mod` commands and appear with destination ranges in the inspector; ninth route rejected. CLAP keeps the stable subset explicitly allowed by spec13. Controller end-to-end regression. |
| OI-09 | Spec 15 | Creative effects: DAW smoke and listening tests | External | Real DAW hosts, USB interface |
| OI-10 | Spec 13, 15 | VST3 validator runs, DAW automation | External | `scripts/verify_vst3.py` on real hosts |
| OI-11 | HARDWARE-ACCEPTANCE.md | Audio device, MIDI, latency, dropout, unplug tests | External, NOT RUN | Physical hardware |
| OI-12 | Spec 16 | Euclidean rhythm command | Done 2026-10-21 | `tests/euclid.rs` |
| OI-13 | Spec 17 | Bitcrusher effect | Done 2026-10-21 | `tests/bitcrusher.rs` |
| OI-14 | Audit 2026-10-12 | Confirm DLL/plugin scanning in two VST3 and two CLAP hosts | External | Host runs |
| OI-15 | Audit 2026-10-21 | Dead public `Command::Tempo` (see audit F-02) | Done 2026-10-22 | Phase-preserving audio-thread tempo handoff, pending note-offs and quantized edits/scenes, fractional BPM, pause/restart, coherent telemetry, and zero-allocation regressions. |

## Notes
* Spec 16 and 17 are new; their specs were written before the code.
* Discrete effect parameters (`bitcrusher` bits/downsample, existing `delay` sync, and similar) are not lockable by existing design.

Tempo commands (`tempo BPM`, alias `bpm BPM`) preserve musical phase at the audio-thread handoff. Pause/resume retains that phase; restart begins at beat zero while keeping BPM. Pending musical edits and note-offs follow the new clock; an already armed resampling interval keeps its exact physical frame bounds. Beat-repeat grid changes fade the previous bounded held slice to dry before fresh capture. Black-box recording retains timestamped CLI `EditorCommand` attempts and emits the existing validated `TempoChange { bpm }` event only after the controller accepts the tempo handoff.
