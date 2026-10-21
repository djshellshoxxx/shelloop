# Developer Guide

Rust toolchain (stable). Linux real-time audio needs ALSA headers (`libasound2-dev` on Debian and Ubuntu).

```sh
cargo build
cargo test --all-targets                 # core: 460 tests pass across the workspace on this snapshot
cargo build --release --features realtime-audio,midi,terminal-ui
cargo test --all-targets --features realtime-audio,midi,terminal-ui
```

The CLAP plugin lives in `clap-plugin/` (its own crate, with host tests in `clap-plugin/tests/host.rs`). Release pipeline: `.github/workflows/release.yml`. Hardware latency and controller checks are separate (`docs/HARDWARE-ACCEPTANCE.md`), so a passing build does not establish them.
