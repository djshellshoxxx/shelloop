# SHELLOOP

SHELLOOP is a terminal-only real-time music instrument: a live step sequencer plus a playable synthesizer designed for Windows and Linux.

Development is active on `continuation/rebuild-baseline` in draft PR #1. The deterministic musical core, hardware-facing playable synth, live pattern-playback path and real-time-safe WAV recording foundation are implemented on this branch. The beta tag is still intentionally withheld pending physical audio/MIDI validation and final release verification.

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
- Background WAV writer and preallocated real-time recording bridge
- Experimental PC-speaker probe: Linux console speaker ioctl and Windows Beep compatibility mode
- Bounded recording queues and master-output protection
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
  --record <FILE>        Record the mono master output to a 32-bit float WAV
  --pc-speaker-test      Play a short experimental PC-speaker probe tone and exit
  --pc-speaker-frequency <HZ>  Probe frequency, 37-32767 Hz (default: 440)
  --pc-speaker-duration <MS>   Probe duration, 1-5000 ms (default: 250)
  --bpm <TEMPO>          Set sequencer tempo from 20 to 400 (default: 120)
  --steps-per-beat <N>   Set sequencer grid density from 1 to 64 (default: 4)
  --list-devices         List available audio and MIDI devices
  --mouse-xy             Enable terminal mouse XY performance control
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

Enable mouse XY Performance Mode:

```bash
shelloop --mouse-xy
```

With a pattern loaded, hold the left mouse button and drag inside the terminal. X crossfades between the live synth (left) and sequencer (right); Y controls overall performance level. Without a loaded pattern, X is ignored and Y controls the live synth level. Mouse capture is disabled again when Shelloop exits.

Record the protected mono master output while performing:

```bash
shelloop --record take.wav
```

Record a pattern performance:

```bash
shelloop --no-midi --pattern patterns/example-bassline.json --bpm 138 --record bassline-take.wav
```

Recording uses the actual selected output device sample rate, writes 32-bit float WAV data on a background writer thread, and reports dropped/rejected blocks when the session ends.


### Experimental PC-speaker probe

To test whether Shelloop can invoke the platform PC-speaker path:

```bash
shelloop --pc-speaker-test
```

Choose a different frequency or duration:

```bash
shelloop --pc-speaker-test --pc-speaker-frequency 880 --pc-speaker-duration 300
```

On Linux, Shelloop attempts the kernel console PC-speaker tone ioctl through `/dev/console` or `/dev/tty0`. The machine must actually have a PC speaker/buzzer and the process must have permission to access the console device.

On Windows, this uses the Win32 `Beep` compatibility API. Modern Windows normally routes that sound through the default audio device rather than directly driving a motherboard speaker, so Shelloop reports it as compatibility output instead of claiming physical BIOS-speaker access.

`--list-devices` also reports the platform PC-speaker backend under **Special outputs**.

## Windows: complete setup and testing guide

Shelloop targets 64-bit Windows. There are two ways to run it: use the prebuilt `v0.01-beta` package once that release is published, or build the current development branch from source now.

### Option A: use the prebuilt Windows beta

Once `v0.01-beta` is published, open the repository's **Releases** section and download:

```text
shelloop-v0.01-beta-windows-x86_64.zip
shelloop-v0.01-beta-windows-x86_64.zip.sha256
```

Extract the ZIP to a normal writable folder, for example:

```text
C:\Tools\shelloop\
```

The extracted directory will contain at least:

```text
shelloop.exe
README.md
LICENSE
patterns\
```

Open PowerShell in that directory. In File Explorer you can click the address bar, type `powershell`, and press Enter.

Confirm that the executable starts:

```powershell
.\shelloop.exe --help
```

List the audio and MIDI devices visible to Shelloop:

```powershell
.\shelloop.exe --list-devices
```

Then start with the Windows default audio output and the first available MIDI input:

```powershell
.\shelloop.exe
```

If no MIDI keyboard is connected, start with MIDI disabled:

```powershell
.\shelloop.exe --no-midi
```

The `v0.01-beta` release is not published yet. Until the release gate is complete, use the source-build instructions below.

### Option B: build the current version from source

Shelloop is written in Rust and uses the normal Windows MSVC Rust toolchain.

#### 1. Install Git

Install Git for Windows if `git` is not already available. Confirm it from PowerShell:

```powershell
git --version
```

#### 2. Install the Microsoft C++ build tools

Install **Visual Studio 2022 Build Tools** or Visual Studio 2022 with the **Desktop development with C++** workload enabled. The Rust MSVC target needs the Microsoft linker and Windows SDK supplied by this workload.

After installation, a normal PowerShell window should be sufficient once Rust is installed.

#### 3. Install Rust

Install Rust using `rustup` and use the stable MSVC toolchain. After installation, close and reopen PowerShell, then verify:

```powershell
rustc --version
cargo --version
rustup show
```

The active host/toolchain should be the 64-bit MSVC target, normally:

```text
x86_64-pc-windows-msvc
```

If required, set it explicitly:

```powershell
rustup default stable-x86_64-pc-windows-msvc
```

#### 4. Clone Shelloop

Choose a working folder and clone the repository:

```powershell
cd $HOME\Documents
git clone https://github.com/djshellshoxxx/shelloop.git
cd shelloop
```

The current playable development work is on:

```text
continuation/rebuild-baseline
```

Switch to it:

```powershell
git fetch origin
git switch continuation/rebuild-baseline
```

Confirm the branch:

```powershell
git branch --show-current
```

It should print:

```text
continuation/rebuild-baseline
```

#### 5. Run the automated tests

Start with the core test suite:

```powershell
cargo test --all-targets
```

Then test the actual Windows runtime feature set:

```powershell
cargo test --all-targets --features realtime-audio,midi,terminal-ui
```

Run strict Clippy checks as well:

```powershell
cargo clippy --all-targets --features realtime-audio,midi,terminal-ui -- -D warnings
```

These are the same main Rust checks used by the Windows CI job.

#### 6. Build the optimized executable

Build the release version with audio, MIDI and terminal UI enabled:

```powershell
cargo build --release --features realtime-audio,midi,terminal-ui
```

The executable will be created at:

```text
target\release\shelloop.exe
```

Test it:

```powershell
.\target\release\shelloop.exe --help
```

### Windows audio setup

Shelloop uses CPAL to access the Windows audio system. For the first test, set the Windows output device you want to use as the normal Windows default output, then run:

```powershell
.\target\release\shelloop.exe --no-midi
```

To see the exact device names Shelloop can select:

```powershell
.\target\release\shelloop.exe --list-devices
```

Use the audio device name exactly as Shelloop prints it:

```powershell
.\target\release\shelloop.exe --audio-device "YOUR EXACT DEVICE NAME" --no-midi
```

For example, if the device list contains `Speakers (Focusrite USB Audio)`, run:

```powershell
.\target\release\shelloop.exe --audio-device "Speakers (Focusrite USB Audio)" --no-midi
```

If a USB interface is not listed, confirm that Windows can play ordinary system audio through it first, then close and reopen Shelloop after reconnecting the interface.

### Windows MIDI setup

Connect the MIDI keyboard/controller before starting Shelloop, then list devices:

```powershell
.\target\release\shelloop.exe --list-devices
```

Select a MIDI port by exact name:

```powershell
.\target\release\shelloop.exe --midi-name "YOUR MIDI PORT NAME"
```

Or select it by the zero-based index shown in the device listing:

```powershell
.\target\release\shelloop.exe --midi-index 0
```

A useful first hardware test is:

```powershell
.\target\release\shelloop.exe --audio-device "YOUR AUDIO DEVICE" --midi-name "YOUR MIDI PORT" --polyphony 24
```

Play several notes at once, use the sustain pedal, release notes in different orders, and disconnect/reconnect the MIDI controller. Shelloop's MIDI reconnect path sends panic/all-notes-off when a connection disappears so stale notes should not remain sounding.

### Test without a MIDI controller

The computer keyboard can play the synth directly. Start Shelloop without MIDI:

```powershell
.\target\release\shelloop.exe --no-midi
```

The keyboard layout is:

```text
Lower row: Z S X D C V G B H N J M
Upper row: Q 2 W 3 E R 5 T 6 Y 7 U
```

Use:

```text
[     octave down
]     octave up
!     panic / all notes off
~     quit
Esc   quit
```

Windows supplies key press, repeat and release events directly, so computer-keyboard note releases should behave more naturally than terminals that only report key presses.

### Test the sequencer

An example pattern is included in the repository. Run it at 120 BPM:

```powershell
.\target\release\shelloop.exe --no-midi --pattern .\patterns\example-bassline.json --bpm 120 --steps-per-beat 4
```

Or at a faster tempo:

```powershell
.\target\release\shelloop.exe --no-midi --pattern .\patterns\example-bassline.json --bpm 138 --steps-per-beat 4
```

While a pattern is loaded:

```text
Space       play / pause
Backspace   restart pattern from frame zero
!           panic / all notes off
Esc or ~    quit
```

You can still play the live synth over the sequencer because live performance and sequenced playback use separate synth paths.

### Recommended first Windows QA session

For the first real-machine test, use this order:

1. Run `--help` and confirm the executable starts.
2. Run `--list-devices` and confirm the intended audio device is shown.
3. Run `--no-midi` and test computer-keyboard notes.
4. Test octave up/down and panic.
5. Load `patterns\example-bassline.json` and test play, pause and restart.
6. Connect a MIDI controller and confirm note-on/note-off behavior.
7. Test chords up to the selected polyphony value.
8. Test sustain-pedal press/release.
9. Disconnect and reconnect the MIDI controller while Shelloop is running.
10. Let it run continuously and listen for crackles, dropouts, stuck notes, timing jumps or pitch changes.
11. Repeat using the desired USB audio interface instead of the Windows default device.

For beta validation, note the audio interface model, MIDI controller model, Windows version, sample rate shown/used by the device, and any audible glitch or unexpected console message.

### Useful Windows commands

List devices:

```powershell
.\target\release\shelloop.exe --list-devices
```

Default audio, no MIDI:

```powershell
.\target\release\shelloop.exe --no-midi
```

Specific audio output:

```powershell
.\target\release\shelloop.exe --audio-device "DEVICE NAME" --no-midi
```

Specific audio and MIDI devices:

```powershell
.\target\release\shelloop.exe --audio-device "DEVICE NAME" --midi-name "MIDI PORT" --polyphony 24
```

Pattern playback:

```powershell
.\target\release\shelloop.exe --pattern .\patterns\example-bassline.json --bpm 138 --steps-per-beat 4 --no-midi
```

Run tests:

```powershell
cargo test --all-targets --features realtime-audio,midi,terminal-ui
```

Build release executable:

```powershell
cargo build --release --features realtime-audio,midi,terminal-ui
```

### Windows troubleshooting

**`cargo` or `rustc` is not recognized**

Close and reopen PowerShell after installing Rust. If it still fails, verify that `%USERPROFILE%\.cargo\bin` is on the user PATH.

**Linker errors such as `link.exe not found`**

Install or modify Visual Studio 2022 Build Tools and enable **Desktop development with C++**, including the Windows SDK. Then reopen PowerShell and build again.

**No audio device appears**

Verify that Windows recognizes the device and can play normal system audio through it. Disconnect/reconnect USB audio hardware, then restart Shelloop and run `--list-devices` again.

**The wrong audio device is used**

Run `--list-devices`, copy the exact printed name, and pass it to `--audio-device` in quotes.

**No MIDI port appears**

Connect and power on the controller before running `--list-devices`. Close DAWs or utilities that may have exclusive access to the MIDI device and try again.

**A MIDI port disconnects during use**

Leave Shelloop running and reconnect it. The runtime polls for MIDI device changes and attempts to reconnect the selected target. It also sends panic when the connection is lost to reduce the chance of stuck notes.

**Notes are too loud, distorted or clipped**

Reduce the Windows/interface output volume while testing. Shelloop applies master-output protection, but hardware gain staging still matters.

**Crackles or dropouts**

Close high-CPU applications, avoid Bluetooth audio for latency testing, use a wired/USB interface where possible, and test again. Record the device model and circumstances because physical latency/xrun tuning remains part of the beta QA gate.

**PowerShell blocks execution**

Shelloop is an `.exe`, not a PowerShell script, so PowerShell execution-policy changes should normally not be required. Run it with an explicit relative path such as `.\shelloop.exe` or `.\target\release\shelloop.exe`.

**Windows SmartScreen warns about the beta executable**

The first beta may not yet have an established code-signing reputation. Verify that the file came from this repository's Releases page and compare its SHA-256 hash with the published `.sha256` file before running it.

To calculate the downloaded ZIP hash yourself:

```powershell
Get-FileHash .\shelloop-v0.01-beta-windows-x86_64.zip -Algorithm SHA256
```

Compare that value with:

```text
shelloop-v0.01-beta-windows-x86_64.zip.sha256
```

### Current Windows beta limitations

The project is still pre-beta. Automated Windows CI proves that the all-feature code compiles, tests, passes strict Clippy and produces an optimized executable, but CI cannot prove real speaker output, real MIDI-controller behavior, end-to-end latency, xrun/dropout behavior or sound quality on physical hardware.

Live recording is now wired through `--record <FILE>`. The current beta records the protected mono master mix; multichannel/stem recording is not implemented yet.

The final `v0.01-beta` release will only be tagged after the remaining real-hardware QA checks and release-package verification are complete.

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

`[` and `]` shift octave, `!` sends panic/all-notes-off, and `~` or `Esc` quits. With a pattern loaded, Space toggles sequencer play/pause and Backspace restarts from frame zero. `--mouse-xy` enables left-button drag performance control, using X as live/sequencer crossfade and Y as overall level. Windows provides key press/repeat/release events directly. On Unix-like terminals, Shelloop requests the crossterm/kitty keyboard enhancement protocol so notes can be released correctly. If the terminal does not support it, Shelloop falls back to bounded timed note releases and reports that limitation at startup.

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

The WAV recording layer uses a background writer thread and a separate preallocated real-time bridge. Full buffers are passed to the writer with nonblocking bounded-channel operations, then cleared and recycled back into the pool instead of allocating a new audio block for every callback buffer.

## Beta release pipeline

The package version is `0.1.0-beta.1`. The eventual `v0.01-beta` tag triggers Windows x86-64 and Linux x86-64 optimized builds with all runtime features, packages the binary with this README, MIT license and example patterns, emits SHA-256 checksum files, and publishes a GitHub prerelease.

The release workflow can also be run manually for package verification without publishing a tag. Normal pull requests use the cross-platform CI workflow for formatting, tests, strict Clippy, all-feature checks and optimized runtime builds; the release-packaging workflow is intentionally kept off ordinary PR commits so development feedback is not duplicated.

The tag is intentionally not created yet. Remaining release gates are physical Windows/Linux playback and MIDI-controller validation, latency/xrun/sound-quality checks, device-unavailable and unplug/reconnect validation on real hosts, live-recording validation on real hardware, and verification of the tag-triggered packaging workflow against the exact final release commit.

## v1 direction

- Quantized live pattern and scene replacement
- Terminal command mode and richer full-screen performance display
- Terminal mouse XY performance control
- Drum synthesis and WAV sample playback
- Mixer sends, delay/reverb/saturation and metering
- Expanded project/preset schema for patterns, scenes, mappings and audio settings
- Windows and Linux first; macOS evaluated separately

## Status

Automated CI validates deterministic logic, runtime feature builds and optimized binaries. Physical playback, MIDI hardware, latency and sound-quality validation cannot be claimed from CI and remain required before the beta tag is published.
