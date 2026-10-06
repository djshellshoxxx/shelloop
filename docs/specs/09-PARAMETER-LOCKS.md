# Spec 09 — Per-step Parameter Locks

## Objective

Allow individual sequencer steps to temporarily override synth, sample, mixer and effect parameters, creating Elektron-style parameter automation without requiring a continuous automation timeline.

## User outcome

A single bass step can open the filter, one snare can have more delay, one sample step can reverse or transpose, and values return deterministically after the locked step unless another lock takes over.

## Core principle

Parameter locks are musical events compiled with the pattern. They are not UI callbacks and they do not mutate project base values permanently.

## Lock addressing

Reuse stable parameter IDs:

```rust
pub struct ParameterLock {
    pub target: LockTarget,
    pub value: ParamValue,
}

pub enum LockTarget {
    Track(TrackParamId),
    Synth(SynthParamId),
    Sample(SampleParamId),
    Effect { slot: EffectSlotId, param: EffectParamId },
}
```

Locks may target only parameters explicitly marked `lockable` by their parameter descriptor. Structural parameters such as changing effect type, sample asset allocation, polyphony capacity or project track count are not lockable.

## Pattern schema

Each active step gains a bounded lock list:

```rust
pub struct PatternStep {
    // existing fields...
    pub locks: SmallLockSet,
}
```

Maximum initial locks per step: 16. Pattern validation rejects more rather than allocating arbitrary callback state.

## Semantics

At the sample frame where a step triggers:

1. Resolve base parameter state for the track.
2. Apply that step's lock values.
3. Trigger the note/sample event.
4. Locked values remain effective for the step's lock lifetime.
5. At the next relevant step boundary, parameters without a continuing/new lock return toward base state using their defined smoothing.

Default lock lifetime: until the next sequencer step for that track. This is independent of note gate duration.

Future lock lifetime modes may include gate-duration or explicit duration, but v1 uses next-step semantics to keep restoration deterministic.

## Base value changes while locked

If MIDI learn/TUI changes a base parameter while a lock is active, the new base value is stored immediately but does not replace the currently sounding locked value. When the lock expires, restoration targets the newest base value.

This distinction is mandatory:

```text
base value = persistent/current patch state
effective value = base plus current temporary lock override
```

## Multiple locks

At most one lock for the same target is allowed on one step. Duplicate target entries fail validation or are coalesced by the editor before compile.

If ratchets retrigger the same step, the same lock remains active; it is not repeatedly pushed onto a stack.

## Scene/pattern transitions

Changing pattern or scene clears lock ownership from the old pattern at the exact activation frame and resolves effective values from the new active pattern/base state. No old lock may leak across a pattern replacement.

Pause behavior: current effective values may remain frozen while paused, but `stop`, `restart` and panic must restore/clear transient lock state according to explicit engine tests. Recommended v1 behavior: pause freezes; stop/panic restores base.

## Sample-accurate event ordering

For note triggers sharing the same frame as lock events, locks apply before note-on so pitch/filter/sample parameters affect that trigger. At restoration boundaries, restoration occurs before new locks for the new step, then new locks overwrite where present.

Define and test a stable event precedence enum rather than relying on insertion order.

## Editor/TUI

Selected step displays lock count and values. Commands:

```text
lock filter.cutoff 4800
lock filter.resonance 0.7
lock fx1.delay.mix 0.5
unlock filter.cutoff
locks
```

Copy/paste step includes locks. Clearing a note step should define whether locks remain as a trigless lock. v1 should support `trigless` parameter-lock steps explicitly because they are musically useful: a step may have no note but still apply locks.

## Trigless locks

Represent step content independently as optional trigger + lock collection. A lock-only step applies parameter changes without note-on. Probability may optionally gate the entire step including locks; v1 default: probability governs both trigger and locks so deterministic probability produces one coherent event decision.

## Tests

Write failing tests first for:

- Lock applies before note-on at identical frame.
- Lock restores to base at next track step.
- New base value received during lock becomes restoration target.
- Consecutive locks on same parameter transition directly without transient base glitch.
- Trigless lock changes parameter without note event.
- Probability false suppresses both trigger and locks.
- Ratchets do not duplicate lock-stack semantics.
- Pattern/scene replacement clears old lock state at boundary.
- Stop/panic restores base.
- Maximum locks per step is enforced.
- Non-lockable structural parameter is rejected.
- Behavior is identical across different audio callback block sizes.

## Completion criteria

Pattern editor/TUI can add/remove locks on supported parameters, locks are persisted, compiled and applied sample-accurately, base/effective values remain separate, and no transient lock state leaks across stop, panic, pattern replacement or scene changes.