# Shelloop v0.01-beta QA Record

Date: 2026-10-04
Branch: `continuation/rebuild-baseline`
Verified code head: `27be5e679b8080c741401e17f9707489900b57ae`
GitHub Actions run: `37190843336` (CI run #91)

## Automated verification

The following checks completed successfully against the verified code head.

### Default/core job — Ubuntu

- `cargo fmt --all -- --check`
- `cargo test --all-targets`
- `cargo clippy --all-targets -- -D warnings`
- dependency resolution completed and the generated `Cargo.lock` was preserved as the `cargo-lock` workflow artifact

Result: PASS.

### Full runtime — Windows

Features: `realtime-audio,midi,terminal-ui`

- `cargo check --all-targets --features realtime-audio,midi,terminal-ui`
- `cargo test --all-targets --features realtime-audio,midi,terminal-ui`
- `cargo clippy --all-targets --features realtime-audio,midi,terminal-ui -- -D warnings`
- `cargo build --release --features realtime-audio,midi,terminal-ui`

Result: PASS.

### Full runtime — Ubuntu Linux

Features: `realtime-audio,midi,terminal-ui`

- ALSA development headers installed in the job
- `cargo check --all-targets --features realtime-audio,midi,terminal-ui`
- `cargo test --all-targets --features realtime-audio,midi,terminal-ui`
- `cargo clippy --all-targets --features realtime-audio,midi,terminal-ui -- -D warnings`
- `cargo build --release --features realtime-audio,midi,terminal-ui`

Result: PASS.

## Runtime behavior covered by automated tests

- audio sample clipping and non-finite-value sanitization
- mono-to-interleaved output behavior and invalid channel/buffer rejection
- deterministic audio device-name selection
- renderer construction at the selected device's actual sample rate
- MIDI port selection by first port, index, and exact case-insensitive name
- MIDI reconnect decision logic
- MIDI note-on/note-off routing to real-time synth commands
- sustain pedal routing and all-notes-off/panic routing
- malformed MIDI decoding rejection
- keyboard note mapping, octave bounds, panic and quit controls
- startup option parsing, conflicts, invalid values, help and error exit behavior
- bounded voice allocation, sustain release, voice stealing and panic
- deterministic pattern probability, independent pattern lengths, swing, microtiming and ratchets
- sample-frame quantization and transport invariants
- project validation and atomic persistence

## Runtime hardening review

The hardware-facing runtime uses bounded control and MIDI queues. The audio callback owns the synth and does not perform terminal rendering or filesystem I/O. Synth construction occurs after CPAL reports the selected device sample rate. Audio stream errors are reported through a bounded channel.

Normal quit, terminal read/poll failures after audio startup, audio stream errors and terminal setup failures request `EngineCommand::Panic` before session teardown. MIDI target disappearance also requests panic before the old connection is dropped, reducing the risk of stuck voices.

The production-code panic/unwrap audit found no explicit `panic!` calls. The remaining production `expect` in `VoiceAllocator::choose_slot` is protected by the constructor invariant that polyphony is always in `1..=256`; an allocator therefore always contains at least one slot.

## Terminal input behavior

Windows uses crossterm key press/repeat/release events. On compatible Unix terminals, Shelloop requests keyboard enhancement event types so note-off can follow real key release. Where release events are unavailable, the runtime reports the limitation and uses bounded timed note releases rather than allowing indefinitely stuck keyboard notes.

## Physical validation still required

CI cannot validate real audio hardware or subjective/audio-timing behavior. Before publishing `v0.01-beta`, perform at least the following on representative Windows and Linux machines:

- enumerate audio devices and select both default and explicitly named outputs
- verify stable playback at the device's native/default sample rate
- hold/release chords from the computer keyboard and confirm no stuck notes
- connect a physical MIDI keyboard/controller and verify notes, sustain and panic
- unplug/reconnect the active MIDI device and confirm panic/reconnect behavior
- verify terminal state is restored after normal quit and forced runtime errors
- check audible latency, xruns/underruns and sound quality under sustained polyphony
- validate no-audio-device and unavailable-selected-device errors on real hosts

## Known scope remaining beyond the playable-synth runtime

The deterministic pattern scheduler exists and is tested, but live sequencer/pattern playback is not yet connected to the real-time audio session. The broader v1 design also still includes scenes, richer terminal UI/command mode, mouse XY control, drum/sample playback, effects, and the WAV recorder writer thread.

## Release status

The release workflow is present and configured to package Windows and Linux optimized binaries with README/LICENSE and SHA-256 checksums when tag `v0.01-beta` is pushed. The release tag has not been created. Publishing remains gated on the physical validation above and completion of the beta scope selected for live sequencing.
