# Spec 07 — Effects Engine

## Objective

Add a bounded stereo effects architecture for track inserts, shared sends and master processing. Initial musical effects are saturation/distortion, delay and reverb, with metering-compatible signal boundaries.

## User outcome

Tracks can be shaped and spatialized without external plugins. A bass can be saturated, a lead sent to delay, drums sent to reverb, and the master protected without breaking real-time guarantees.

## Signal architecture

```text
Instrument/sample track
  -> track pre-gain
  -> insert slots (0..N)
  -> pan/track fader
  -> dry master bus
  -> send taps

Send buses
  -> bus effect chain
  -> return gain
  -> master bus

Master bus
  -> master inserts
  -> protection/limiter boundary
  -> recorder/output
```

v1 maximums:

- 4 insert slots per track.
- 2 send buses.
- 2 effects per send bus.
- 4 master insert slots.
- Limits are compile-time/project validation constraints, not vectors that grow in the callback.

## Effect interface

```rust
pub trait StereoEffect {
    fn process(&mut self, left: f32, right: f32) -> (f32, f32);
    fn set_param(&mut self, param: EffectParamId, normalized: f32);
    fn reset(&mut self);
}
```

Runtime implementation may prefer an enum dispatch instead of trait objects if it improves predictability and avoids dynamic allocation. The public contract is stable effect type + stable parameter IDs.

## Effect types

### Saturation / distortion

Modes: soft clip, hard clip, tanh-style saturation or a small stable set. Parameters: drive, tone (optional one-pole filter), mix, output gain.

### Delay

Stereo delay with independently addressable or linked left/right times. Parameters:

- Time in milliseconds or tempo-synced division.
- Feedback 0–safe maximum below self-oscillation unless an explicit guarded mode is added.
- Wet/dry.
- Optional ping-pong.
- Optional feedback low-pass.

Maximum delay time defines a preallocated circular buffer. Default maximum 4 seconds per channel.

Tempo-synced delay converts division to samples from current BPM and sample rate. Time changes crossfade/interpolate read position to avoid abrupt clicks where practical.

### Reverb

Implement a bounded algorithmic reverb suitable for no-allocation processing: Schroeder/Freeverb-derived or another stable topology. Parameters: size, decay, damping, pre-delay, wet/dry, width. No convolution/file loading in v1.

## Effect bypass

Bypass state changes are smoothed/crossfaded over a fixed short interval to avoid clicks. Bypass does not destroy internal state unless `reset` is requested.

## Send model

Each track has send levels for Send A and Send B. Send source defaults post-fader; a project field may later add pre-fader. Send buffers are accumulated once per frame into fixed stereo bus values.

## Parameter IDs

Each effect exposes a typed static parameter descriptor:

```rust
pub struct EffectParamDescriptor {
    pub id: EffectParamId,
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub curve: ParamCurve,
}
```

This descriptor drives MIDI learn, TUI editors and parameter locks.

## CPU and memory budgets

Effect construction/allocation occurs off the callback or during engine snapshot construction. Delay/reverb storage is preallocated. Project validation computes required effect memory and enforces a documented budget.

No FFT/convolution in the first version.

## Stereo transition

Spec 01 establishes an internal stereo mixer. Recording remains backward-compatible: current `--record` may continue recording mono until recording schema is upgraded, but the effects implementation should expose stereo master frames so future stereo/stem recording does not require another mixer rewrite.

## Error behavior

Invalid parameter values are rejected/clamped at validated boundaries according to the parameter contract. Unknown effect types in a project are load errors, not silently bypassed, unless a future compatibility mode explicitly supports placeholders.

## Tests

Write failing tests first for:

- Every effect returns finite samples for valid finite input/parameter extremes.
- Saturation gain/mix endpoints.
- Delay impulse appears at expected sample index.
- Delay buffer wrap is correct.
- Feedback remains finite at allowed maximum.
- Tempo sync recalculates correctly after BPM change.
- Bypass transition is click-reduced and converges to dry.
- Reverb tail decays instead of growing without bound.
- Send A/B routing does not leak between tracks/buses.
- Master effect ordering is stable.
- Memory limits reject oversized configurations before activation.
- Effect parameter changes are deterministic across audio block partitions.

## Completion criteria

A project can place saturation, delay and reverb in bounded track/send/master routes; parameters are addressable through stable IDs; multiple tracks can share send effects; all-feature CI remains clean; and hardware QA finds no obvious clicks, runaway feedback, NaN output or callback allocation.