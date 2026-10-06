# Spec 06 — MIDI Learn and Persistent Control Mapping

## Objective

Allow physical MIDI controllers to bind knobs, faders, buttons, pads and pedals to stable Shelloop parameters without hardcoded controller profiles.

## User outcome

The performer enters learn mode for a parameter, moves a hardware control once, and Shelloop creates a persistent mapping. Future MIDI messages update the target parameter with optional range, inversion, curve, pickup and button behavior.

## Prerequisites

This spec depends on stable parameter identifiers from the synth, mixer, transport, effects and scene systems. UI labels must never be the persistence key.

## Address model

```rust
pub enum ParameterTarget {
    Global(GlobalParamId),
    Track { track: TrackId, param: TrackParamId },
    Synth { track: TrackId, param: SynthParamId },
    Effect { track: Option<TrackId>, slot: EffectSlotId, param: EffectParamId },
}

pub struct MidiSource {
    pub port_match: MidiPortMatch,
    pub channel: Option<u8>,
    pub message: MidiControlMessage,
}

pub enum MidiControlMessage {
    ControlChange { controller: u8 },
    Note { note: u8 },
    PitchBend,
}
```

`MidiPortMatch` supports exact stored port name plus a future-friendly optional manufacturer/device fingerprint. A mapping may be configured as any-port only when the user explicitly chooses that behavior.

## Mapping model

```rust
pub struct MidiMapping {
    pub id: MappingId,
    pub source: MidiSource,
    pub target: ParameterTarget,
    pub min: f32,
    pub max: f32,
    pub curve: MappingCurve,
    pub inverted: bool,
    pub pickup: PickupMode,
    pub button: ButtonMode,
}
```

Mapping curves: linear and logarithmic initially. Targets with discrete values use explicit discrete conversion rather than rounding arbitrary floats.

## Learn workflow

1. User selects a parameter and invokes `learn`.
2. Control layer enters `AwaitingMidiSource { target, deadline }`.
3. The next eligible MIDI control message is captured, not applied as a normal performance command unless explicitly configured.
4. Shelloop presents the proposed source/target mapping.
5. Mapping becomes active immediately after confirmation or automatically in fast-learn mode.
6. Project dirty state is updated.

Default learn timeout: 15 seconds. Escape/cancel exits without changing mappings.

Note-on musical keyboard events are excluded from learning by default to avoid accidentally binding played notes. The user may explicitly enable note/pad learn.

## Pickup / soft takeover

Knobs/faders should not cause parameter jumps when hardware and software values differ.

`PickupMode`:

- `Jump`: incoming value immediately controls target.
- `Pickup`: ignore motion until the hardware value crosses the current software value.
- `Scale`: relative takeover maps remaining hardware travel onto remaining software range.

Default for continuous CC: Pickup. Default for buttons/pads: Jump.

Pickup state is control-thread state and is reset when a mapping target changes externally in a way that invalidates the previous crossing condition.

## Button modes

- Momentary: press=on, release=off.
- Toggle: each press toggles.
- Trigger: press emits one action and has no persistent numeric state.

Actions such as scene launch, play/pause, restart and panic are mapped as commands rather than pretending they are numeric parameters.

## MIDI normalization

CC maps 0–127 to normalized 0–1. Pitch bend maps its decoded signed range to normalized 0–1 or bipolar -1–1 according to target requirements. MIDI messages are validated by the existing decoder.

## Real-time path

MIDI mapping lookup occurs outside or at the edge of the real-time engine. The mapping table is an immutable compiled snapshot. Incoming MIDI creates bounded typed engine commands. No map mutation, JSON work or string lookup occurs inside the audio callback.

## Conflict policy

One physical control may map to multiple targets only when explicitly allowed. Default learn behavior detects an existing mapping from the same source and asks/requires one of: replace, add secondary, cancel.

Multiple sources may map to the same target.

## Persistence

Mappings are stored in the project schema with stable target IDs, source descriptors and mapping behavior. If a target no longer exists, project load reports the orphan mapping and keeps it disabled rather than redirecting it by track index.

## CLI/TUI commands

Examples:

```text
learn track 2 filter.cutoff
learn master.gain
learn scene.next
unlearn 14
mappings
mapping 14 range 200 8000
mapping 14 invert on
mapping 14 pickup pickup
```

## Tests

Write failing tests first for:

- CC normalization endpoints and midpoint.
- Linear/log/inverted range mapping.
- Pickup ignores values until crossing target.
- Jump mode applies immediately.
- Toggle reacts once per press and not to repeated release messages.
- Learn captures correct source and does not mutate target before completion.
- Learn timeout/cancel leaves mappings unchanged.
- Existing-source conflict follows explicit replacement policy.
- Deleted track leaves mapping orphaned, never retargeted.
- Port disconnect/reconnect preserves mapping definition.
- Compiled mapping lookup is deterministic and bounded.
- Mapping queue overflow is counted/reported without blocking MIDI callback/audio.

## Completion criteria

A generic MIDI controller can learn at least mixer gain/pan, synth cutoff/resonance, transport play/pause, and scene launch controls; mappings persist with the project; reconnecting the same device restores them; and no learned-control activity can introduce blocking work into the audio callback.