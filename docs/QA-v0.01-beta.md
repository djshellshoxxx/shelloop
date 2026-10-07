# Shelloop v0.01-beta QA Record

Date: 2026-10-05
Branch: `continuation/rebuild-baseline`
Verified product-code head: `5b205d4577c845f309f8388f12355476ba8ce774`
GitHub Actions CI run: `37277967104` (CI run #114)
GitHub Actions release-build run: `37277967094` (Release run #6)

## Automated verification

The following checks completed successfully against the verified product-code head after the final CLI/version QA fix.

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

## Release-package QA

Release run `37277967094` rebuilt and packaged both release targets from the verified product-code head.

### Linux x86_64

- all-feature release-source tests: PASS
- optimized release build: PASS
- package verifier: PASS
- workflow artifact: `linux-x86_64`
- archive: `shelloop-v0.01-beta-linux-x86_64.tar.gz`
- SHA-256 checksum verification after download: PASS
- artifact type: 64-bit x86_64 ELF PIE executable
- `shelloop --version`: `shelloop 0.1.0-beta.1`
- `shelloop --help` contains `-V, --version`: PASS
- invalid option returns exit code 2 and an explicit error: PASS
- README, LICENSE, and `patterns/example-bassline.json` present: PASS

### Windows x86_64

- all-feature release-source tests: PASS
- optimized release build: PASS
- package verifier: PASS
- workflow artifact: `windows-x86_64`
- archive: `shelloop-v0.01-beta-windows-x86_64.zip`
- SHA-256 checksum verification after download: PASS
- artifact type: PE32+ x86_64 Windows console executable
- README, LICENSE, and `patterns/example-bassline.json` present: PASS

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
- startup option parsing, conflicts, invalid values, help, version and error exit behavior
- bounded voice allocation, sustain release, voice stealing and panic
- deterministic pattern probability, independent pattern lengths, swing, microtiming and ratchets
- validated JSON pattern loading
- live sample-frame sequencer note-on and gate-timed note-off generation
- sequencer pause/resume position semantics and restart-to-zero behavior
- sample-frame quantization and transport invariants
- project validation and atomic persistence

## Live sequencer integration

Live pattern playback is connected to the hardware-facing CPAL session. Pattern files are read and validated before the audio stream starts. The callback owns a `LiveSequencer` and a dedicated sequencer synth, separate from the keyboard/MIDI performance synth.

The separation means a sequencer pause, restart or sequencer panic does not terminate a note currently held from the computer keyboard or a MIDI controller. Space toggles sequencer playback and Backspace restarts at frame zero when a pattern is loaded.

Pattern scheduling uses a preallocated event cache and bounded pending note-off storage. Sequencer transport commands are delivered over a fixed-capacity channel. The callback performs no pattern-file filesystem I/O.

## Runtime hardening review

The hardware-facing runtime uses bounded performance, MIDI and sequencer-control queues. The audio callback owns the synth state and does not perform terminal rendering or filesystem I/O. Synth and sequencer construction occurs after CPAL reports the selected device sample rate. Audio stream errors are reported through a bounded channel.

Normal quit, terminal read/poll failures after audio startup, audio stream errors and terminal setup failures request panic before session teardown. MIDI target disappearance also requests panic before the old connection is dropped, reducing the risk of stuck voices.

The production-code panic/unwrap audit found no explicit `panic!` calls. The remaining production `expect` in `VoiceAllocator::choose_slot` is protected by the constructor invariant that polyphony is always in `1..=256`; an allocator therefore always contains at least one slot.

## Terminal input behavior

Windows uses crossterm key press/repeat/release events. On compatible Unix terminals, Shelloop requests keyboard enhancement event types so note-off can follow real key release. Where release events are unavailable, the runtime reports the limitation and uses bounded timed note releases rather than allowing indefinitely stuck keyboard notes.

## Physical validation status

Automated QA cannot validate subjective audio quality or real external hardware behavior. The following checks remain recommended for beta testers on representative Windows and Linux systems:

- enumerate audio devices and select both default and explicitly named outputs
- verify stable playback at the device's native/default sample rate
- load `patterns/example-bassline.json` and verify stable sequencer playback
- verify Space pause/resume and Backspace restart without cutting held live-performance notes
- hold/release chords from the computer keyboard and confirm no stuck notes
- connect a physical MIDI keyboard/controller and verify notes, sustain and panic
- play MIDI/keyboard notes while the sequencer runs and confirm both paths coexist correctly
- unplug/reconnect the active MIDI device and confirm panic/reconnect behavior
- verify terminal state is restored after normal quit and forced runtime errors
- check audible latency, xruns/underruns and sound quality under sustained polyphony plus sequencer load
- validate no-audio-device and unavailable-selected-device errors on real hosts

These are documented as beta hardware-validation items rather than silently claimed as completed.

## Scope after the beta release

The selected beta code scope is implemented and covered by automated CI and artifact-level package QA. Broader v1 work remains, including quantized live pattern/scene replacement, richer terminal UI and command mode, mouse XY control, drum/sample playback, effects and the WAV recorder writer thread.

## Release status

The release workflow packages Windows and Linux optimized binaries with README/LICENSE and SHA-256 checksums for `v0.01-beta`. Automated CI, cross-platform release builds, package verification and independent artifact smoke checks are green. The release is explicitly a beta; physical audio/MIDI hardware validation remains an open beta-testing item.
