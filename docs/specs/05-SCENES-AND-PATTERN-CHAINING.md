# Spec 05 — Scenes and Pattern Chaining

## Objective

Add quantized scene launching and deterministic scene chains so Shelloop can move from repeating loops to structured live arrangements.

## User outcome

A project can contain scenes such as Intro, Verse, Build, Drop and Break. Launching a scene swaps the intended patterns/track states together on a musical boundary. A chain can automatically advance through scenes for a configured number of bars/repeats.

## Scene model

```rust
pub struct SceneId(pub u16);

pub struct Scene {
    pub id: SceneId,
    pub name: String,
    pub track_states: Vec<SceneTrackState>,
}

pub struct SceneTrackState {
    pub track_id: TrackId,
    pub pattern_id: PatternId,
    pub muted: Option<bool>,
    pub gain: Option<f32>,
}
```

A scene is a sparse override: omitted track properties remain unchanged. Every referenced track/pattern must resolve during compilation.

## Scene launch

`LaunchScene { scene_id, boundary }` creates one atomic compiled scene change. All included tracks change at the same absolute frame. It is invalid for one track to apply a scene one audio block later than another.

Default launch boundary: next bar. Supported boundaries reuse `QuantizeBoundary`.

## Pattern library

Tracks gain stable `PatternId` values and may own multiple patterns. The active pattern is just one selection from the library. Editing a pattern that is not active is permitted on the control side.

## Chain model

```rust
pub struct ChainStep {
    pub scene_id: SceneId,
    pub repeats: u16,
}

pub struct SceneChain {
    pub steps: Vec<ChainStep>,
    pub loop_chain: bool,
}
```

`repeats` means number of scene-length cycles/bars according to a defined chain clock. For v1, chain duration is expressed in bars to avoid ambiguity from unequal pattern lengths. Future versions may support follow actions based on pattern completion.

## Manual override

Manual scene launch while a chain is running has an explicit policy:

- Default: stop chain and launch requested scene.
- Optional later mode: temporary override then resume.

No hidden resume behavior in v1.

## Follow actions

Not required for first implementation, but schema should reserve an optional follow action: `Next`, `RandomWeighted`, `Stop`. Random behavior must be seeded/deterministic if added.

## Scene state and UI feedback

Expose:

- Active scene.
- Queued scene.
- Frame/bar at which queued scene will activate.
- Chain active/paused state.
- Current chain step and repetitions remaining.

The UI may display a queued scene immediately but must distinguish it from active.

## Commands

Examples:

```text
scene launch drop
scene launch break bar
scene next
scene prev
chain start main
chain stop
chain pause
```

Keyboard shortcuts may later map number/function keys to scene slots.

## Persistence

Scenes and chains are project-level objects. IDs are stable; names are user-facing and need not be unique unless the command parser requires disambiguation. If duplicate names are allowed, CLI commands must support IDs.

## Tests

Write failing tests first for:

- Two or more track pattern replacements activate at exactly the same frame.
- Queued scene does not alter active state early.
- Scene launch on an exact boundary applies once.
- Scene containing invalid track/pattern ID is rejected before enqueue.
- Manual launch stops active chain according to policy.
- Chain repeat counts advance exactly at bar boundaries.
- Looping chain returns to first step without duplicate launch.
- Stop/panic/restart semantics are explicit and deterministic.
- Editing an inactive pattern does not affect active audio.
- Scene changes remain deterministic across different callback block sizes.

## Completion criteria

A project can define at least 32 scenes and chains of at least 128 entries on the control side, while the callback only retains bounded compiled state required for the active/queued transitions. Scene launch remains sample-frame deterministic and does not cut unrelated live-performance voices unless a scene explicitly requests that behavior.