# Spec 01 — Multi-track Engine

## Objective

Replace the current single sequencer/single sequenced-synth runtime with a bounded multi-track performance engine. A project may contain synth, sample and later external-MIDI tracks, each with an independent pattern length, mute/solo state, gain, pan, instrument state and quantized pattern replacement.

## User outcome

A performer can run drums, bass, lead and texture tracks simultaneously, mute/solo them independently, give each a different loop length, and continue playing the live keyboard synth without transport interruption.

## Scope

Initial implementation supports 1–16 tracks. Track count is fixed for a running project snapshot. Track creation/deletion occurs on the control side and is applied through a quantized immutable replacement. The audio callback never resizes the track vector.

Track kinds:

- `Synth` — owns a `RealtimeSynth` plus pattern sequencer.
- `Sample` — reserved contract for Spec 04; may initially validate but remain unavailable until that spec lands.
- `ExternalMidi` — reserved schema value; audio generation is not part of this phase.

## Core data model

```rust
pub struct TrackId(pub u16);

pub enum TrackKind { Synth, Sample, ExternalMidi }

pub struct TrackDefinition {
    pub id: TrackId,
    pub name: String,
    pub kind: TrackKind,
    pub gain: f32,        // 0.0..=2.0
    pub pan: f32,         // -1.0..=1.0
    pub muted: bool,
    pub soloed: bool,
    pub pattern: Pattern,
}

pub struct EngineProjectSnapshot {
    pub revision: u64,
    pub tracks: Arc<[CompiledTrackDefinition]>,
}
```

`TrackId` is stable inside a project and must not be inferred from vector position. IDs are allocated on the control side and persisted.

## Real-time representation

`RealtimeTrack` owns only prevalidated/preallocated objects required for rendering. No track name, file path or UI-only strings are required in the callback.

```rust
struct RealtimeTrack {
    id: TrackId,
    sequencer: LiveSequencer,
    instrument: RealtimeInstrument,
    gain: SmoothedParam,
    pan: SmoothedParam,
    muted: bool,
    soloed: bool,
}
```

Maximums for v1:

- 16 tracks.
- 256 steps per pattern.
- Existing ratchet maximum remains 8.
- Per-track scheduled-event cache is preallocated.
- Control queue overflow does not block; newest UI action may be rejected with a visible counter/error.

## Mixer semantics

The internal mixer becomes stereo even before the effects spec. Each mono instrument is equal-power panned into left/right. Track gain is applied before pan. Solo logic is evaluated once per callback/control revision, not by allocating or scanning arbitrary collections per sample.

Gain/pan changes are smoothed over a small fixed interval to avoid zipper noise. Mute immediately suppresses new audio but must not corrupt voice state. Unmute resumes current state. A future option may choose panic-on-mute, but it is not the default contract.

## Transport and timing

All tracks share the project sample-frame transport and BPM. Pattern lengths remain independent. A 15-step bass track may run against a 16-step drum track without forced reset. Restart resets all track sequencers to frame zero atomically.

Pattern replacement uses existing `QuantizedChange<T>` semantics. Supported boundaries: immediate, step, beat, bar, target-pattern end.

## Commands

Add bounded control commands:

```rust
enum TrackCommand {
    SetGain { track: TrackId, gain: f32 },
    SetPan { track: TrackId, pan: f32 },
    SetMute { track: TrackId, muted: bool },
    SetSolo { track: TrackId, soloed: bool },
    Panic { track: Option<TrackId> },
    ReplacePattern { track: TrackId, change: QuantizedChange<CompiledPattern> },
}
```

Invalid IDs are rejected on the control side and safely ignored/countable in the callback if a stale command still arrives.

## CLI/project compatibility

`--pattern FILE` remains supported and constructs an implicit one-track project. New project files may define multiple tracks. Do not require users of the current CLI to migrate immediately.

## Project schema

Project schema gains:

```json
{
  "schema_version": 2,
  "tracks": [
    {
      "id": 1,
      "name": "Bass",
      "kind": "synth",
      "gain": 0.8,
      "pan": 0.0,
      "mute": false,
      "solo": false,
      "pattern": {}
    }
  ]
}
```

Loading schema v1 creates one compatible synth track. Saving writes the newest schema unless an explicit migration tool is later added.

## Tests

Write failing tests first for:

- Two tracks produce events at independent loop lengths.
- Track ordering does not change deterministic probability outcomes.
- Mute/solo truth table with zero, one and multiple soloed tracks.
- Equal-power pan endpoints and center behavior.
- Gain/pan smoothing produces finite bounded samples.
- Quantized pattern replacement affects only the target track.
- Global restart resets every track exactly once.
- Global panic silences all voices.
- Stale/missing `TrackId` cannot panic.
- Sixteen-track maximum is accepted; seventeen is rejected before runtime.
- Single-track compatibility produces the same scheduled frames as the old path.

## Completion criteria

The runtime can load a multi-track project, render at least four simultaneous synth tracks in CI simulation, expose mute/solo/gain/pan through testable commands, and preserve the existing single-pattern CLI path. Physical QA listens for timing stability, clicks during gain/pan changes, and CPU/dropout behavior with 16 active tracks.