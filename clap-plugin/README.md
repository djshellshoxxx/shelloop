# SHELLOOP CLAP plugin

SHELLOOP's polyphonic synthesizer (16 voices) and built-in step sequencer,
packaged as a [CLAP](https://cleveraudio.org/) instrument plugin
(`com.circuitdriftlabs.shelloop`, vendor Circuit Drift Labs).

- **Notes:** one note input port that accepts CLAP note events and MIDI 1.0
  (note on/off, CC64 sustain, CC120/CC123 all-off). Events are sample-accurate.
- **Audio:** one stereo 32-bit float output. The mono synth signal goes to
  both channels.
- **Sequencer:** set **Sequencer** to *On* and start the host transport to
  play the bundled `patterns/example-bassline.json`. It follows the host
  tempo and song position (four steps per beat). When you stop the transport,
  the sequencer releases its notes.
- **State:** parameter values are saved as a small versioned JSON blob.
- **Latency:** 0 samples.

## Parameters

| Id | Name              | Range                                      |
|----|-------------------|--------------------------------------------|
| 0  | Oscillator        | Sine / Triangle / Saw / Pulse              |
| 1  | Octave            | -4 .. +4                                   |
| 2  | Fine              | -100 .. +100 cents                         |
| 3  | Pulse width       | 5 % .. 95 %                                |
| 4  | Amp attack        | 0 .. 10 s                                  |
| 5  | Amp decay         | 0 .. 10 s                                  |
| 6  | Amp sustain       | 0 .. 100 %                                 |
| 7  | Amp release       | 0 .. 10 s                                  |
| 8  | Filter mode       | Bypass / Low-pass / High-pass / Band-pass  |
| 9  | Cutoff            | 20 Hz .. 20 kHz (capped at 0.45 × sample rate) |
| 10 | Resonance         | 0 .. 1                                     |
| 11 | Filter env amount | -8 .. +8 octaves                           |
| 12 | Output gain       | 0 .. 2 (synth patch gain)                  |
| 13 | Sequencer         | Off / On                                   |
| 14 | Master volume     | 0 .. 2 (smoothed, after the synth)         |

The ids are stable, so host automation keeps working across versions.

## Build

```sh
cargo build --release -p shelloop-clap
```

## Install

**Linux:** copy and rename the shared library into your CLAP folder:

```sh
mkdir -p ~/.clap
cp target/release/libshelloop_clap.so ~/.clap/shelloop.clap
```

**Windows:** copy `target\release\shelloop_clap.dll` to
`C:\Program Files\Common Files\CLAP\shelloop.clap`:

```powershell
Copy-Item target\release\shelloop_clap.dll "C:\Program Files\Common Files\CLAP\shelloop.clap"
```

Then rescan plugins in your host.

## Test

```sh
cargo test -p shelloop-clap
```

`tests/host.rs` is a small in-process host. It calls the exported
`clap_entry` directly and covers the factory and descriptor, the extensions,
sample-accurate notes, parameter text, saving and loading state, the
transport-synced sequencer, and activation.
