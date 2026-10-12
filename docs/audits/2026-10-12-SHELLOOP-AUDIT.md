# SHELLOOP repository audit — 2026-10-12

## Audit method and scope
Reviewed current default-branch README, Cargo workspace manifests, CI workflow, VST3 README, PR descriptions and branch names through the linked GitHub repository. This is a repository **triage audit**, not a claim that a checkout was built or all source files were inspected. CI logs and actual real-device tests have not been reproduced in this session.

## Verified repository statements
- Default-branch README describes all 12 original specifications as implemented, including MIDI learn, resampling, scenes/chains, parameter locks, effects, TUI and pattern variation.
- Root Cargo manifest defines the main runtime plus CLAP workspace member. The CLAP crate currently exports cdylib, staticlib and rlib.
- CI matrix includes Linux and Windows backend tests, release builds, CLAP builds and a VST3 build/verification job.
- VST3 wrapper uses clap-wrapper with a Rust CLAP plugin static library.
- PR #5 reports 413 passing workspace tests and multiple fixed regression categories *at its test time*; this is historical evidence, not a current run.
- PR #6 describes untested Windows VST3 linking and a lack of a real-DAW load test at submission time.
- The README has an outdated Windows source-build branch instruction pointing to `continuation/rebuild-baseline` rather than `main`.

## Risks and incomplete acceptance
1. Verify latest PR #6 merge state and Windows/Linux VST3 job outcomes before tagging another release.
2. Confirm DLL/plugin factory scanning in at least two VST3 hosts and two CLAP hosts; plugin factory verification alone is not DAW acceptance.
3. Complete `docs/HARDWARE-ACCEPTANCE.md` using actual Windows and Linux audio hardware and MIDI devices.
4. Test host sample-rate/block-size changes, suspend/resume, device unplug, all-notes-off, automation and project/state reload.
5. Probe worst-case DSP performance and real-time allocations under maximum sample voices, effect chains, dense ratchets and simultaneous pattern changes.
6. Confirm correct version/tag/release asset parity and SHA-256 verification.
7. Audit remaining stale README guidance and branch references after branch/PR resolution.

## Outstanding work decision
Treat the original v1 features as *reported complete*, but not fully acceptance-verified. Complete build/host/hardware validation first; implement expansion specs 13–15 in additive modules on feature branches after regression coverage. This branch contains specifications only, not new DSP implementations.

## Branch hygiene
Do not merge `continuation/*` by branch name alone; compare each against `main` to distinguish already-merged history from unique commits. Do not merge branches from active workers without reviewing their latest changes and checks. Require passing CI on every feature PR.

## Next audit execution checklist
Obtain a fresh checkout; inventory every module and `TODO/unimplemented!` marker; run `cargo fmt --all -- --check`, `cargo test --workspace --all-targets --all-features`, strict clippy and release builds on Linux and Windows; inspect build logs and fix failures with failing-before/passing-after regression tests; run VST3 and CLAP plug-in validators; perform long-running audio stress tests; then review DSP numerical and realtime safety. Record commands, OS/toolchain, result hashes, and remaining risks.
