# Spec 15 — Creative modulation effects and sound-design performance controls

Status: proposed, not implemented.

## Goal
Extend saturation, delay and reverb with distinctive musical movement suited to electronic performance, without compromising reliable live playback.

## First release
**Tempo-synced multimode filter**: state-variable LP/BP/HP, continuous cutoff, Q with safe resonance limit and optional envelope-follow modulation.

**Stereo chorus/flanger**: one shared bounded stereo delay buffer per effect, fractional interpolation, independent stereo LFO phase, feedback clamp, wet/dry balance, and optional tempo synchronization.

**Beat repeat / stutter**: quantized fixed-capacity capture buffer, step-length subdivisions, repeat count, reverse and gated randomization. A repeat never begins from uninitialized or stale buffer data; playback mode and latency behavior are defined for stop/panic and project switches.

**Performance macros**: expose two assignable parameters per effect chain and support safe recall on scene launch. Compatible with Spec 13 once implemented.

## Integration
Use existing insert/send/master effect slots rather than adding hidden chains. Each module implements the existing effect processing contract, with `prepare(sample_rate,max_block)`, `process(stereo_frame,context)`, `reset`, and typed parameter update. Allocate all delay/stutter memory during preparation; account for it in the documented per-project effects memory budget. Replace DSP instances through the existing bounded control-to-audio handoff and retire old resources on the control thread. Avoid allocations, locks, runtime FFT creation, filesystem access and unbounded loops inside the callback.

## Parameter semantics
All numeric parameters have units, defaults, bounded valid ranges, stable identifiers, smoothing and bypass policies. Feedback coefficients must be strictly below unity in magnitude. Reject NaNs/infinities. Avoid jumps at effect activation with wet/dry smoothing and buffers primed to silence; do not silently change the existing v1 saturation/delay/reverb algorithms or preset sound.

## Acceptance
Automated: impulse response and silence tests, DC/finite checks, maximum-feedback stability, sample-rate change, determinism for fixed seed and transport, tempo subdivision precision, channel isolation, no RT allocations, memory/callback duration measurement, legacy project compatibility, and test renders with fixtures. Manual: Windows/DAW CLAP and VST3 smoke tests, USB interface and MIDI controller acceptance, presets at both slow and fast tempos.

## Phases
Phase A: reusable modulation/timing primitives and effect parameter schema. Phase B: filter + chorus/flanger with golden renders. Phase C: beat repeat and quantized transport scheduling. Phase D: interface, preset examples, automation and documentation. Release only after cross-platform CI and real-host validation.
