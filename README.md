# SHELLOOP

SHELLOOP is a terminal-only real-time music instrument: a live step sequencer plus a playable synthesizer designed for Windows and Linux.

Development is active on `continuation/rebuild-baseline` in draft PR #1. The deterministic musical core and the first hardware-facing playable-synth runtime have been reconstructed and verified. Live pattern playback is the next major runtime integration step, so this branch is not yet tagged as the beta release.

## Implemented

- Sample-frame transport and exact in-block event offsets
- Deterministic pattern scheduling with independent lengths, probability, swing, microtiming and ratchets
- Fixed-capacity polyphonic voice allocation with deterministic stealing, sustain and panic/all-voices-off
- Real-time oscillator synth rendering at the actual CPAL device sample rate
- CPAL audio-device enumeration, default/named-device selection and guarded output streams
- MIDI 1.0 note/CC/pitch-bend decoding and `midir` input enumeration/connection
- Live MIDI note, sustain and panic routing into the synth
- MIDI hot-plug polling/reconnect with panic on disconnect to avoid stuck notes
- Computer-keyboard performance mapping, octave controls, panic and quit
- Crossterm key-release reporting where supported, with a timed note-off fallback on older terminals
- Bounded control/MIDI queues and no blocking terminal or filesystem work in the audio callback
- Project validation and atomic JSON persistence
- Bounded recording queue and master-output protection
- Windows/Linux all-feature compile, test, strict Clippy and optimized-release CI gates
- `v0.01-beta` packaging workflow for Windows x86-64 and Linux x86-64 with SHA-256 checksums

## Command line

```text
Usage: shelloop [OPTIONS]

Options:
  --audio-device <NAME>  Select an audio output device by exact name
  --midi-name <NAME>     Select a MIDI input port by exact name
  --midi-index <INDEX>   Select a MIDI input port by zero-based index
  --polyphony <VOICES>   Set synth polyphony from 1 to 256 (default: 16)
  --list-devices         List available audio and MIDI devices
  --no-midi              Disable MIDI input
  -h, --help             Print help
```

List devices before starting:

```bash
shelloop --list-devices
```

Start with the default audio device and first MIDI input:

```bash
shelloop
```

Select hardware explicitly:

```bash
shelloop --audio-device "Focusrite USB" --midi-name "Launchkey MIDI" --polyphony 24
```

Run without MIDI:

```bash
shelloop --no-midi
```

## Live keyboard controls

The playable synth uses a two-row piano layout:

```text
Lower: Z S X D C V G B H N J M
Upper: Q 2 W 3 E R 5 T 6 Y 7 U
```

`[` and `]` shift octave, `!` sends panic/all-notes-off, and `~` or `Esc` quits. Windows provides key press/repeat/release events directly. On Unix-like terminals, Shelloop requests the crossterm/kitty keyboard enhancement protocol so notes can be released correctly. If the terminal does not support it, Shelloop falls back to bounded timed note releases and reports that limitation at startup.

## Build

Install a current stable Rust toolchain. Linux builds with real-time audio require ALSA development headers.

Core-only build and tests:

```bash
cargo build
cargo test --all-targets
```

Full runtime build:

```bash
cargo build --release --features realtime-audio,midi,terminal-ui
```

Full runtime tests:

```bash
cargo test --all-targets --features realtime-audio,midi,terminal-ui
```

On Debian/Ubuntu:

```bash
sudo apt-get install libasound2-dev
```

## Real-time design

The CPAL callback owns the `RealtimeSynth`. UI and MIDI threads communicate with it through a fixed-capacity channel. The renderer is constructed only after CPAL reports the selected device's actual sample rate, preventing pitch/timing errors caused by assuming 44.1 or 48 kHz. Each sample drains only a bounded number of commands before rendering, and callback error reporting uses a bounded channel.

MIDI input is also bounded. Malformed messages and queue overflows are counted instead of panicking. The terminal loop periodically re-enumerates MIDI devices; if the selected target disappears it requests panic before dropping the connection and reconnects when the target returns.

## Beta release pipeline

The package version is `0.1.0-beta.1`. The eventual `v0.01-beta` tag triggers Windows x86-64 and Linux x86-64 optimized builds with all runtime features, packages the binary with this README and MIT license, emits SHA-256 checksum files, and publishes a GitHub prerelease.

The tag is intentionally not created yet. Remaining release gates include live pattern/sequencer playback integration and physical audio/MIDI hardware validation for latency, disconnect behavior and sound quality.

## v1 direction

- Live step sequencer playback using the existing deterministic pattern scheduler
- Quantized pattern and scene replacement while audio is running
- Terminal command mode and richer full-screen performance display
- Terminal mouse XY performance control
- Drum synthesis and WAV sample playback
- Mixer sends, delay/reverb/saturation and metering
- WAV recorder writer thread around the existing bounded recording queue
- Expanded project/preset schema for patterns, scenes, mappings and audio settings
- Windows and Linux first; macOS evaluated separately

## Status

Automated CI validates deterministic logic, runtime feature builds and optimized binaries. Physical playback, MIDI hardware, latency and sound-quality validation cannot be claimed from CI and remain required before the beta tag is published.
