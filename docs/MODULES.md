# Module index

Purpose, entry points and invariants of the modules added for the sound-design expansion (specs 13–15). Older modules keep their header comments in `src/`. Dependencies point downwards; nothing here depends on the UI, filesystem, network or an audio device.

| Module | Responsibility | Public entry points | Depends on |
|---|---|---|---|
| `src/modulation/model.rs` | Serialisable, validated modulation data: LFOs, macros, ≤8 routes. A default config serialises to nothing. | `ModulationConfig`, `ModRoute`, `LfoConfig`, `ModSource`, `ModDest`, `SyncDivision` | `serde` |
| `src/modulation/compile.rs` | Control-thread compile into a `Copy` struct with coefficients precomputed. | `CompiledModulation::compile` | `model` |
| `src/modulation/runtime.rs` | Per-voice LFO phase, control-rate evaluation (every 16 samples), interpolated offsets. No allocation. | `ModRuntime`, `ModInputs`, `ModOffsets`, `lfo_value` | `compile` |
| `src/modulation/command.rs` | `mod ...` text commands, atomic apply, inspector range text. | `parse_mod_command`, `apply_mod_command`, `route_range_text` | `model` |
| `src/sample_shaping/slices.rs` | Slice model, validation, equal division, offline transient detection, fixed-size `SliceTable` for the audio thread. | `SliceSettings`, `SliceTable`, `equal_slices`, `detect_slices` | `sample::StereoFrame` |
| `src/sample_shaping/shaping.rs` | Shaping settings, compiled form and per-voice DSP (transient shaper → drive → filter). | `SampleShaping`, `CompiledShaping`, `VoiceShaping` | `filter`, `mixer`, `envelope` |
| `src/sample_shaping/command.rs` | `sample ...` / `slice ...` commands, atomic apply. | `parse_sample_command`, `apply_sample_command` | `shaping`, `slices` |
| `src/fx_creative.rs` | Filter, chorus/flanger and beat-repeat DSP states owned by `effects::Effect`. | crate-internal (`FilterFx`, `ChorusFx`, `BeatRepeatFx`) | `filter`, `mixer` |
| `src/effect_macros.rs` | Two macros per effect chain, validation and per-scene recall compilation. | `EffectMacro`, `validate_effect_macros`, `compile_scene_recalls` | `effects`, `scenes` |

## Integration seams

* `synth.rs` — `SynthPatch.modulation` (optional field), `SynthVoice` applies offsets only when a route is active, so unmodulated rendering is bit-identical to the legacy path (`tests/modulation.rs`).
* `engine.rs` — `RealtimeSynth` keeps the shared free-running LFO phase, tempo, mod wheel (`EngineCommand::ModWheel`, MIDI CC 1) and the averaged Pan/EffectWet offsets that `multitrack.rs` applies to the track strip.
* `sample.rs` — `SampleSettings.{sample_shaping,slices}` are optional; `SampleVoice` copies its region at note-on and owns its DSP state; `RealtimeSampler::{set_shaping,set_slices}` are the live-edit entry points (one `AudioMessage::SampleConfig`).
* `effects.rs` — three new `EffectKind`s; v1 saturation/delay/reverb are untouched.
* `controller.rs` — `mod`, `sample`/`slice` and `fxmacro` commands; all validate first, send one bounded message, then commit the control snapshot.
* `clap-plugin/` — parameters 15–22 appended; ids 0–14 frozen by `original_parameter_ids_are_frozen`.

## Real-time invariants

* No allocation, lock, I/O or logging in `process`/`next_sample`/`handle` of any module above; enforced by `tests/realtime_allocations.rs` with a counting global allocator.
* Every fixed capacity has a documented limit: 8 routes, 2 LFOs, 2 macros, 128 slices, 64 sample voices, 2 effect macros per chain, beat-repeat buffers sized for one beat at 20 BPM.
* Slice tables, shaping and modulation are `Copy` so nothing needs freeing on the audio thread.

## Migration notes

* Existing projects, patches, presets and CLAP states load unchanged; new JSON blocks are optional and omitted when default.
* `EngineCommand` gained `ModWheel`; `SynthParamId` and `SampleParamId` gained variants (existing names unchanged); `MultiTrackProject` gained the optional `effect_macros` field.
