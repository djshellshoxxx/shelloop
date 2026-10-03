# SHELLOOP

SHELLOOP is a terminal-only real-time music instrument: a live step sequencer plus a playable synthesizer designed for Windows and Linux.

The project is being rebuilt from its approved engineering specification after an earlier implementation session was not persisted to GitHub.

## v1 direction

- Continuous real-time synthesis while sequencing and editing
- Sample-frame transport and exact in-block event offsets
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

Reconstruction in progress on the continuation branch. Hardware playback/latency validation remains a release gate and will not be claimed from CI alone.
