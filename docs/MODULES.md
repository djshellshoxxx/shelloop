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
| `src/music/scale.rs` | Scale kinds, pitch-class and note-name parsing, nearest-in-scale snapping. | `Scale`, `ScaleKind`, `parse_note`, `parse_pitch_class` | none |
| `src/music/chord.rs` | Chord qualities and voicings with inversions, MIDI range checked. | `chord_notes`, `ChordQuality` | `scale` (names only) |
| `src/music/arp.rs` | Arpeggio cycles (up, down, updown, random); `note_at` is a pure function of the index. | `Arpeggio`, `ArpMode` | `chord` |
| `src/music/euclid.rs` | Maximally even (Euclidean) rhythms with rotation. | `euclidean`, `MAX_EUCLID_STEPS` | none |
| `src/music/humanize.rs` | `Humanize` model (timing and velocity jitter) and keyed randomness (SplitMix64). | `Humanize`, `keyed_unit`, `splitmix64` | none |
| `src/music/ratio.rs` | Step ratio conditions `P:Q`. | `StepRatio`, `ratio_fires`, `parse_ratio` | none |
| `src/music/tap.rs` | Tap tempo from recent intervals, 20..400 BPM. | `TapTempo` | none |
| `src/music/edit.rs` | Pure pattern transforms (quantize, transpose, Euclid, chord, arp). Each returns a validated pattern for the editor to commit. | `quantize_notes`, `transpose_pattern`, `apply_euclid`, `apply_chord`, `apply_arp` | `scale`, `chord`, `arp`, `euclidean`, `pattern` |
| `src/music/command.rs` | Text syntax for the musical commands; parse only. | `parse_music_command`, `MusicCommand` | the modules above |
| `src/metronome.rs` | Click synthesis and count-in clock, sample accurate, allocation free. | `Metronome`, `CountIn` | none |
| `src/autosave.rs` | Autosave policy (interval, dirty check) and the sidecar path. Writing happens in the controller. | `AutosavePolicy`, `autosave_path` | none |

## Integration seams

* `synth.rs` — `SynthPatch.modulation` (optional field), `SynthVoice` applies offsets only when a route is active, so unmodulated rendering is bit-identical to the legacy path (`tests/modulation.rs`).
* `engine.rs` — `RealtimeSynth` keeps the shared free-running LFO phase, tempo, mod wheel (`EngineCommand::ModWheel`, MIDI CC 1) and the averaged Pan/EffectWet offsets that `multitrack.rs` applies to the track strip.
* `sample.rs` — `SampleSettings.{sample_shaping,slices}` are optional; `SampleVoice` copies its region at note-on and owns its DSP state; `RealtimeSampler::{set_shaping,set_slices}` are the live-edit entry points (one `AudioMessage::SampleConfig`).
* `effects.rs` — three new `EffectKind`s; v1 saturation/delay/reverb are untouched.
* `controller.rs` — `mod`, `sample`/`slice` and `fxmacro` commands; all validate first, send one bounded message, then commit the control snapshot.
* `controller.rs` — musical commands (`scale`, `chord`, `arp`, `euclid`, `humanize`, `ratio`, `tap`, `metronome`, `countin`, `autosave`, `pattern copy|paste|clear|transpose`) enter through `music_command`. Pattern transforms from `music::edit` are committed with `ProjectPatternEditors` (so undo, redo and quantised revisions apply) and queued through `queue_selected`. `tick` runs autosave.
* `pattern.rs` — `Pattern` gained three optional fields: `chords` (extra notes per step), `ratios` (per-step `P:Q` conditions) and `humanize`. All are omitted from JSON when unused. `PatternScheduler` gained a tempo origin (`retimed`, `step_position`, `beat_index_at`); the default origin is zero, so the original grid is unchanged. Chords and humanize are computed per step from global indices, never from the block, so output does not depend on block size.
* `editor.rs` — `remap_step_metadata` carries chords and ratios with their steps on rotate and length changes; `set_step`, `toggle_step` and `clear_pattern` drop chords and ratios on steps left empty.
* `sequencer.rs` — `retime` re-anchors the grid. It rebuilds only the lookahead cache and keeps pending note-offs, so no sounding note is stranded.
* `multitrack.rs` — `retime`, `set_metronome`, `start_count_in`. The engine mixes the click into the master before `protect_master`, only when non-zero. `frames_per_bar` is not changed by retime, so scene and chain bar timing is preserved.
* `clap-plugin/` — parameters 15–22 appended; ids 0–14 frozen by `original_parameter_ids_are_frozen`.

## Real-time invariants

* No allocation, lock, I/O or logging in `process`/`next_sample`/`handle` of any module above; enforced by `tests/realtime_allocations.rs` with a counting global allocator.
* Every fixed capacity has a documented limit: 8 routes, 2 LFOs, 2 macros, 128 slices, 64 sample voices, 2 effect macros per chain, beat-repeat buffers sized for one beat at 20 BPM.
* Slice tables, shaping and modulation are `Copy` so nothing needs freeing on the audio thread.
* Chords, ratios and humanize live in fixed arrays inside `CompiledPattern` (`MAX_CHORD_EXTRA_NOTES` per step). Metronome and count-in keep scalar state only. `tests/realtime_allocations.rs` covers a count-in followed by playback with the metronome on.
* A tempo change never re-emits an event before the change point and never drops one after it (`tests/musical_scheduler.rs`).

## Migration notes

* Existing projects, patches, presets and CLAP states load unchanged; new JSON blocks are optional and omitted when default.
* Musical additions are additive: `Pattern` JSON gains optional `chords`, `ratios` and `humanize` keys, omitted when unused; `AudioMessage` gained `Tempo`, `Metronome` and `CountIn`; `HELP_TEXT` lists the new commands. Scale and tap state and the clipboard are session-only and are not saved.
* The master bus already applies `protect_master` (tanh soft clip); no separate limiter was added.
* `EngineCommand` gained `ModWheel`; `SynthParamId` and `SampleParamId` gained variants (existing names unchanged); `MultiTrackProject` gained the optional `effect_macros` field.
