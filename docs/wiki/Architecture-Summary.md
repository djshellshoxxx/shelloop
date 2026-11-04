# Architecture Summary

From the README and `docs/MODULES.md`.

- **Real-time audio path.** The live pattern JSON plays inside the CPAL audio callback with preallocated scheduling buffers and sample-frame transport (README "Implemented").
- **Bounded engines.** Tracks (1 to 16) and voices use fixed capacity, with deterministic stealing and smoothed gain and pan.
- **Separate synth paths.** Live keyboard and MIDI performance use a different real-time path from sequenced playback.
- **Specs by feature.** Each feature has a spec under `docs/specs/`, and the README lists which are implemented.
- **Plugin as a crate.** The CLAP plugin is a separate Rust crate (`clap-plugin/`), reusing the engine.
- **Acceptance is separate.** Audio hardware and controller validation are tracked outside the code tests (`docs/HARDWARE-ACCEPTANCE.md`).
