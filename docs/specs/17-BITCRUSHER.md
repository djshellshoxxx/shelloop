# Spec 17 — Bitcrusher (bit depth and sample-rate reduction)

Status: implemented 2026-10-21 (see implementation notes). Scope: a new insert/send/master effect kind.

## Goal
Add a lo-fi character effect for drums, breaks and synth textures that needs no buffers, adds no latency and fits the existing effect chain, slots and parameter-lock rules.

## Effect kind
`EffectKind::BitCrusher`, persisted name `bitcrusher`. Existing effect names and parameter names are unchanged.

| Parameter | Range | Default | Curve | Meaning |
|---|---|---|---|---|
| `bits` | 1 .. 16 | 8 | discrete | Signed quantiser depth. Levels are multiples of `2^-(bits-1)`; 16 bits is nearly transparent. |
| `downsample` | 1 .. 32 | 1 | discrete | Sample-and-hold length in frames. 1 means no reduction. |
| `mix` | 0 .. 1 | 1 | linear | Wet/dry balance, smoothed. |

Discrete parameters follow the existing rule (not lockable); `mix` is lockable.

## DSP
1. Every `downsample` frames, each channel is quantised as `round(x * 2^(bits-1)) / 2^(bits-1)` and held.
2. Output is `dry * (1 - mix) + held * mix`.
3. The quantiser does not clip. Inputs in [-1, 1] stay in [-1, 1].
4. Non-finite input is sanitised by the existing effect wrapper; output is always finite.

## Real-time contract
* Memory: zero bytes (no delay lines). `required_memory_bytes` returns 0.
* No allocation, locks, I/O or logging in `process` or `apply`.
* Latency: zero.
* `reset` clears the held samples and hold counter, so stale audio is never replayed.
* Bypass and stopped transport follow the existing effect wrapper (bypass crossfade, silence on reset only through the normal engine path).

## Acceptance
* `bits = 16`, `downsample = 1`: error below 2^-15 for inputs in [-1, 1].
* `bits = 1`: output values are only -1, 0 or 1 for inputs in [-1, 1].
* `downsample = N`: the held value changes at most every N frames and holds exactly N frames.
* `mix = 0` is dry; `reset` clears held state.
* Parameter names/ranges are validated; unknown names and out-of-range values are rejected.
* Round-trip: `bitcrusher` parses from its name and appears in the effect list.
* Zero memory budget, no RT allocations (`tests/realtime_allocations.rs`).

## Implementation notes
* `src/fx_creative.rs` — `BitCrushFx` state (`new`, `apply`, `clear`, `process`) and index constants `bcr::*`.
* `src/effects.rs` — `EffectKind::BitCrusher`, `BITCRUSHER_PARAMS`, state enum dispatch, zero memory accounting.
* Tests: `tests/bitcrusher.rs`.
