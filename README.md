# SHELLOOP

SHELLOOP is a terminal-only real-time music instrument: a live step sequencer plus a playable synthesizer designed for Windows and Linux.

The current continuation branch reconstructs the verified musical core and is now adding the hardware-facing runtime and beta release layer.

## Current implemented core

- Sample-frame transport and exact in-block event offsets
- Oscillator synthesis with finite/bounded output
- Deterministic pattern scheduling with independent lengths, probability, swing, microtiming and ratchets
- Fixed-capacity polyphonic voice allocation and panic/all-voices-off behavior
- MIDI 1.0 note/CC/pitch-bend decoding and MIDI port selection helpers
- CPAL audio-device enumeration/output adapter behind the `realtime-audio` feature
- `midir` input enumeration/connection adapter behind the `midi` feature
- Computer-keyboard performance mapping and octave controls
- Project validation and JSON persistence
- Bounded recording queue and master-output protection
- Command-line startup parsing, help and validation
- Windows/Linux all-feature CI checks

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

The parser is wired into the executable. The full interactive terminal/audio session is still being connected, so the project should not yet be treated as a finished beta.

## Build

Install a current stable Rust toolchain. Linux builds with real-time audio also require ALSA development headers.

Default core build:

```bash
cargo build
cargo test --all-targets
```

Build every runtime backend:

```bash
cargo build --release --features realtime-audio,midi,terminal-ui
```

On Debian/Ubuntu, install the audio development package first:

```bash
sudo apt-get install libasound2-dev
```

## Beta release pipeline

The package version is `0.1.0-beta.1`. Pushing the release tag `v0.01-beta` triggers Windows x86-64 and Linux x86-64 release builds with all runtime features enabled. The workflow packages the binary with this README and the MIT license, creates SHA-256 checksum files, and publishes a GitHub prerelease.

The tag will only be created after the runtime, CI and QA gates are satisfied.

## v1 direction

- Continuous real-time synthesis while sequencing and editing
- Live step sequencing with scenes, probability, ratchets, swing and independent pattern lengths
- Playable synth alongside sequenced tracks
- Terminal command mode and full-screen performance mode
- Computer keyboard, terminal mouse XY and optional MIDI control
- Drum synthesis and WAV sample playback
- Mixer, delay/reverb/saturation, master protection and metering
- Project/preset persistence and WAV recording
- Bounded real-time queues and no blocking I/O in the audio callback
- Windows and Linux first; macOS evaluated separately

## Status

Active development is on `continuation/rebuild-baseline` in draft PR #1. Automated CI can verify deterministic logic and feature builds, but physical audio playback, MIDI hardware, latency and sound-quality validation remain release gates.
