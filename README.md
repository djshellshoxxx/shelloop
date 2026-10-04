# SHELLOOP

SHELLOOP is a terminal-only real-time music instrument: a live step sequencer plus a playable synthesizer designed for Windows and Linux.

Development is active on `continuation/rebuild-baseline` in draft PR #1. The deterministic musical core, hardware-facing playable synth, and first live pattern-playback path are implemented on this branch. The beta tag is still intentionally withheld pending physical audio/MIDI validation and final release verification.

## Implemented

- Sample-frame transport and exact in-block event offsets
- Deterministic pattern scheduling with independent lengths, probability, swing, microtiming and ratchets
- Live JSON pattern playback in the CPAL audio callback with preallocated scheduling buffers
- Space play/pause and Backspace restart controls for loaded patterns
- Separate real-time synth paths for live keyboard/MIDI performance and sequenced playback
- Fixed-capacity polyphonic voice allocation with deterministic stealing, sustain and panic/all-voices-off
- Real-time oscillator synth rendering at the actual CPAL device sample rate
- CPAL audio-device enumeration, default/named-device selection and guarded output streams
- MIDI 1.0 note/CC/pitch-bend decoding and `midir` input enumeration/connection
- Live MIDI note, sustain and panic routing into the synth
- MIDI hot-plug polling/reconnect with panic on disconnect to avoid stuck notes
- Computer-keyboard performance mapping, octave controls, panic and quit
- Crossterm key-release reporting where supported, with a timed note-off fallback on older terminals
- Bounded control/MIDI/sequencer queues and no blocking terminal or filesystem work in the audio callback
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
  --pattern <FILE>       Load a validated JSON pattern for live playback
  --bpm <TEMPO>          Set sequencer tempo from 20 to 400 (default: 120)
  --steps-per-beat <N>   Set sequencer grid density from 1 to 64 (default: 4)
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

Play the included example pattern at 138 BPM:

```bash
shelloop --pattern patterns/example-bassline.json --bpm 138 --steps-per-beat 4
```

Run without MIDI:

```bash
shelloop --no-midi
```

## Pattern JSON

Patterns are validated before the audio stream starts. Each step is either `null` or an object with note, velocity, gate, probability, ratchets and signed microtiming in sample frames. Example:

```json
{
  "name": "Example",
  "seed": 42,
  "swing": 0.08,
  "channel": 1,
  "steps": [
    {"note": 36, "velocity": 0.9, "gate": 0.7, "probability": 1.0, "ratchets": 1, "microtiming_frames": 0},
    null
  ]
}
```

The included `patterns/example-bassline.json` demonstrates probability, ratchets, swing and microtiming.

## Live keyboard controls

The playable synth uses a two-row piano layout:

```text
Lower: Z S X D C V G B H N J M
Upper: Q 2 W 3 E R 5 T 6 Y 7 U
```

`[` and `]` shift octave, `!` sends panic/all-notes-off, and `~` or `Esc` quits. With a pattern loaded, Space toggles sequencer play/pause and Backspace restarts from frame zero. Windows provides key press/repeat/release events directly. On Unix-like terminals, Shelloop requests the crossterm/kitty keyboard enhancement protocol so notes can be released correctly. If the terminal does not support it, Shelloop falls back to bounded timed note releases and reports that limitation at startup.

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

The CPAL callback owns separate `RealtimeSynth` instances for live performance and pattern playback. UI and MIDI threads communicate through fixed-capacity channels. The renderer is constructed only after CPAL reports the selected device's actual sample rate, preventing pitch/timing errors caused by assuming 44.1 or 48 kHz.

Pattern files are read and validated before audio starts. The callback uses a preallocated `LiveSequencer` event cache and pending note-off storage, so pattern scheduling does not perform filesystem work or intentionally grow queues while rendering. Sequencer transport commands use their own bounded channel, allowing a pattern panic/restart without cutting off live keyboard or MIDI voices.

MIDI input is also bounded. Malformed messages and queue overflows are counted instead of panicking. The terminal loop periodically re-enumerates MIDI devices; if the selected target disappears it requests panic before dropping the connection and reconnects when the target returns.

## Beta release pipeline

The package version is `0.1.0-beta.1`. The eventual `v0.01-beta` tag triggers Windows x86-64 and Linux x86-64 optimized builds with all runtime features, packages the binary with this README and MIT license, emits SHA-256 checksum files, and publishes a GitHub prerelease.

The tag is intentionally not created yet. Remaining release gates are fresh automated verification of this sequencer integration, physical Windows/Linux playback and MIDI-controller validation, latency/xrun/sound-quality checks, device-unavailable and unplug/reconnect validation on real hosts, and verification of the tag-triggered packaging workflow against the exact final release commit.

## v1 direction

- Quantized live pattern and scene replacement
- Terminal command mode and richer full-screen performance display
- Terminal mouse XY performance control
- Drum synthesis and WAV sample playback
- Mixer sends, delay/reverb/saturation and metering
- WAV recorder writer thread around the existing bounded recording queue
- Expanded project/preset schema for patterns, scenes, mappings and audio settings
- Windows and Linux first; macOS evaluated separately

## Status

Automated CI validates deterministic logic, runtime feature builds and optimized binaries. Physical playback, MIDI hardware, latency and sound-quality validation cannot be claimed from CI and remain required before the beta tag is published.
