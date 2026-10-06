# Spec 03 — Synth, Filter and ADSR Expansion

## Objective

Turn the current fixed saw-oscillator voice into a usable subtractive synthesizer with user-controllable oscillator, amplitude envelope, filter and filter envelope while maintaining fixed-cost real-time rendering.

## User outcome

Each synth track can sound materially different: bass, lead, pluck, pad, drone or percussion-like patches can be created and saved without external plugins.

## Voice architecture

Per voice signal chain:

```text
Oscillator(s)
   -> oscillator mix
   -> amplitude pre-gain
   -> resonant filter
   -> amplitude envelope
   -> track mixer
```

Initial oscillator waveforms: sine, triangle, saw, square. Noise may be added as a fifth source if implemented with deterministic per-voice PRNG state.

## SynthPatch

```rust
pub struct SynthPatch {
    pub oscillator: Oscillator,
    pub octave: i8,               // -4..=4
    pub semitone: i8,             // -12..=12
    pub fine_cents: f32,          // -100..=100
    pub pulse_width: f32,         // 0.05..=0.95, used by square
    pub amp_env: AdsrParams,
    pub filter: FilterParams,
    pub filter_env: AdsrParams,
    pub filter_env_amount: f32,   // signed octave/semitone mapping
    pub output_gain: f32,
}
```

Patch objects are validated/compiled off-thread. Parameter changes sent during playback update smoothed real-time parameters.

## ADSR

`AdsrParams` fields: attack, decay, sustain, release. Time values use seconds externally and are compiled to coefficients from the actual output sample rate.

Ranges:

- Attack: 0–30 s.
- Decay: 0–30 s.
- Sustain: 0–1.
- Release: 0–60 s.

Zero attack/decay/release is legal and must be click-safe as far as practical without adding hidden long fades. Envelope state machine: Idle, Attack, Decay, Sustain, Release.

Voice stealing resets/retriggers envelope state deterministically. Note-off enters Release unless sustain pedal defers release through the existing allocator semantics.

## Filter

v1 filter modes: low-pass, high-pass, band-pass. Use a stable resonant topology suitable for real-time modulation; implementation may use a state-variable filter or equivalent.

`FilterParams`:

- Mode.
- Cutoff 20 Hz to min(20 kHz, 0.45 * sample_rate).
- Resonance normalized 0–1 with an implementation-defined safe Q ceiling.
- Key tracking 0–1.
- Drive 0–1 optional if it can remain stable/bounded.

Cutoff modulation combines base cutoff + key tracking + filter envelope. Clamp before coefficient calculation.

## Parameter smoothing

User/MIDI changes to cutoff, resonance, gain, tuning and pulse width are smoothed. Envelope stage transitions remain sample-accurate. Parameter smoothing must not allocate.

## Commands and addressing

Define stable parameter IDs instead of wiring UI strings directly into DSP:

```rust
pub enum SynthParamId {
    Oscillator, Octave, Semitone, FineCents, PulseWidth,
    AmpAttack, AmpDecay, AmpSustain, AmpRelease,
    FilterMode, FilterCutoff, FilterResonance, FilterKeytrack,
    FilterAttack, FilterDecay, FilterSustain, FilterRelease,
    FilterEnvAmount, OutputGain,
}
```

These IDs become the common addressing layer for MIDI learn, TUI controls and parameter locks.

## Presets

Project files may embed patches. A later preset directory can save named JSON patches. Unknown future fields are ignored only when schema rules explicitly allow it; invalid known fields produce a validation error.

## Tests

Write failing tests first for:

- ADSR stage transitions at known sample counts.
- Zero-time stages.
- Release begins from current level, not assumed sustain.
- Retrigger and voice stealing semantics.
- Oscillator pitch remains correct at multiple sample rates.
- Filter output remains finite under min/max cutoff/resonance.
- Cutoff clamping respects Nyquist safety.
- Parameter smoothing reaches target and never produces NaN/Inf.
- Patch validation rejects invalid ranges.
- MIDI sustain still interacts correctly with the expanded voice.
- Deterministic noise, if included.

Add DSP property tests where practical: finite input plus valid parameters must produce finite bounded/protected output.

## Completion criteria

Every synth track can load an independent patch, all parameters are addressable by stable IDs, patches can be changed without restarting audio, and CI proves stable finite behavior across supported sample rates. Hardware QA checks clicks, resonance behavior, long releases and CPU use at maximum polyphony.

## Implementation status (2026-10-06)

Implemented: deterministic ADSR and release tails, oscillator/tuning/pulse width,
state-variable filter and envelope modulation, stable parameter ID declarations,
and schema-v2 per-track patch persistence and startup wiring. Legacy projects
without patches retain the original saw sound. Patch objects reject unknown
fields and validate ranges; engine construction revalidates at the device rate.
`projects/example-synth-patches.json` demonstrates three independent patches.

Pending: live patch replacement, parameter-ID dispatch and smoothing of changing
synth parameters, preset directory management, and physical audio/CPU QA. Spec 03
is not complete until those live-control requirements and hardware gates pass.

Continuation validation: Linux stable Rust passed `cargo fmt --all -- --check`,
`cargo test --all-targets` (131 tests), `cargo test --all-targets --all-features`
(130 tests), strict Clippy for both configurations, and `cargo build --release
--all-features`. New regressions cover legacy sound equivalence, independent
track patches, JSON round trips, invalid-field/range rejection, actual-rate
cutoff validation, and finite bounded example rendering at 32/44.1/48/96 kHz.
Windows validation is delegated to the existing CI job; hardware QA remains open.
