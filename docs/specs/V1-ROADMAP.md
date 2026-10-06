# Shelloop v1 Detailed Feature Roadmap

Status: design specification. These documents define the next major Shelloop implementation sequence after the current pre-beta runtime.

## Design goals

Shelloop should become a terminal-native performance workstation rather than a collection of unrelated audio features. New systems must preserve the existing deterministic sample-frame transport, bounded queues, fixed-capacity real-time structures, Windows/Linux support, and clean separation between control/UI work and the CPAL audio callback.

Every implementation phase uses contract-first/TDD development. User input, files, MIDI mappings and project data are validated outside the audio callback. The callback consumes immutable snapshots or bounded commands only. No filesystem access, terminal rendering, blocking locks, unbounded allocation, device enumeration or JSON parsing is allowed in the real-time path.

## Implementation order

1. `01-MULTITRACK-ENGINE.md` — multiple independent sequencer/instrument tracks and a proper mixer boundary.
2. `02-LIVE-PATTERN-EDITOR.md` — edit steps, track parameters and timing while playback continues.
3. `03-SYNTH-FILTER-ADSR.md` — expose an actual subtractive synth voice with oscillator/filter/envelope controls.
4. `04-WAV-SAMPLE-PLAYBACK.md` — bounded one-shot and loop sample playback.
5. `05-SCENES-AND-PATTERN-CHAINING.md` — quantized scene launch and arrangement chains.
6. `06-MIDI-LEARN.md` — runtime MIDI CC/note mapping with persistent project mappings.
7. `07-EFFECTS-ENGINE.md` — bounded insert/send effects and stereo master processing.
8. `08-FULLSCREEN-TUI.md` — ratatui performance/editing workspace over the same engine contracts.
9. `09-PARAMETER-LOCKS.md` — per-step parameter overrides with deterministic restore semantics.
10. `10-RESAMPLING.md` — capture Shelloop output and atomically promote recordings into sample slots.

## Dependency graph

```text
Multi-track engine
   ├── Live pattern editor
   ├── WAV sample playback
   ├── Scenes
   └── Effects routing

Synth/filter/ADSR
   ├── MIDI learn
   └── Parameter locks

Sample playback + recording
   └── Resampling

All engine/control contracts
   └── Full-screen TUI
```

## Compatibility rule

Existing single-pattern CLI behavior remains valid. A project containing one synth track must produce equivalent musical timing to the current runtime. New schemas are additive and versioned. Existing `Pattern`, `LiveSequencer`, `RealtimeSynth`, `QuantizedChange`, recording, MIDI and project primitives should be migrated or extended rather than silently replaced.

## Shared acceptance gate

- `cargo fmt --all -- --check` passes.
- Default and all-feature tests pass on Windows and Linux.
- Strict Clippy passes with warnings denied.
- Release build succeeds on Windows and Linux.
- No new blocking or filesystem calls are introduced into the audio callback.
- Every bounded real-time collection has a documented maximum and overflow policy.
- Every user-editable persistent structure has validation and schema/version behavior.
- Deterministic tests prove timing does not change based on CPAL buffer partitioning.
- Existing keyboard, MIDI, sequencer, recording and mouse-XY behavior remains regression tested.