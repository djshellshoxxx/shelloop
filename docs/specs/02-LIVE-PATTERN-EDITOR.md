# Spec 02 — Live Pattern Editor

## Objective

Allow patterns to be edited while Shelloop is playing without reading/reparsing files inside the audio callback and without corrupting the currently sounding pattern.

## User outcome

The performer can select a track, toggle steps, change note/velocity/gate/probability/ratchets/microtiming, alter pattern length, copy/paste steps and undo mistakes while the transport keeps running.

## Editing model

The editor operates on a mutable control-thread `PatternDraft`. The audio engine only receives validated immutable `CompiledPattern` replacements.

Flow:

```text
keyboard/TUI command
  -> mutate PatternDraft
  -> validate
  -> compile
  -> enqueue QuantizedChange<CompiledPattern>
  -> audio thread applies at requested boundary
```

The currently running pattern never shares mutable memory with the editor.

## PatternDraft

```rust
pub struct PatternDraft {
    pub name: String,
    pub seed: u64,
    pub swing: f32,
    pub channel: u8,
    pub steps: Vec<Option<PatternStep>>, // control thread only
}
```

Control-side vectors may allocate. `compile()` validates the existing bounds and converts to a callback-safe representation.

## Editing operations

Required v1 operations:

- Select track.
- Cursor left/right across steps.
- Toggle step on/off.
- Set note 0–127.
- Set velocity 0.0–1.0.
- Set gate 0.0–1.0.
- Set probability 0.0–1.0.
- Set ratchets 1–8.
- Set signed microtiming.
- Change pattern length 1–256.
- Set swing.
- Copy one step.
- Paste one step.
- Clear step.
- Clear pattern.
- Rotate pattern left/right.
- Duplicate pattern contents.
- Undo/redo.

## Quantization

Every edit has an apply boundary. Defaults:

- Step toggle/value edit: next step.
- Pattern length change: next pattern boundary.
- Clear pattern: next beat unless explicitly immediate.
- Undo/redo: same boundary as the edit type being restored.

Users may later choose `immediate`, `step`, `beat`, `bar`, or `pattern` globally. The engine reuses `QuantizeBoundary`.

## Revision and acknowledgement

Every submitted pattern has a monotonic `edit_revision`. The control layer displays three states: `draft`, `queued`, `active`. The audio/control bridge returns an acknowledgement when a revision becomes active so the TUI never claims an edit is sounding before its quantized boundary.

## Undo/redo

Undo/redo lives entirely outside the audio callback. Maintain a bounded history, default 128 revisions per track. A history entry stores the edited pattern state or a compact reversible operation. When capacity is exceeded, discard the oldest entry.

Undoing a queued-but-not-yet-active edit cancels/replaces the queued revision rather than briefly applying both states.

## Command-mode surface

Examples:

```text
track 2
step 5 toggle
step 5 note 38
step 5 velocity 0.82
step 5 probability 0.6
step 5 ratchets 3
step 5 micro +120
length 15
swing 0.12
rotate right 1
undo
redo
```

Parsing must produce typed edit commands before mutation.

## File persistence

Saving captures the latest draft, including queued edits, after validating it. Loading a replacement file parses off the audio thread and submits it through the same quantized replacement path.

## Error behavior

Invalid values never alter the draft. Validation errors identify track, step and field. Failed compilation leaves the currently active audio pattern unchanged.

## Tests

Write failing tests first for:

- Toggling a step produces a new revision without mutating the active pattern.
- Edit requested mid-step activates exactly at the selected quantization boundary.
- Multiple edits before a boundary coalesce deterministically into the newest compiled revision.
- Undo before activation removes/replaces the pending change.
- Undo after activation creates a valid inverse revision.
- Pattern shortening cannot leave stale scheduled note-offs/events.
- Copy/paste does not alias mutable step state.
- Rotate preserves number and contents of active steps.
- Invalid edits are atomic.
- History is bounded.
- Editor behavior is deterministic regardless of audio block size.

## Completion criteria

A running multi-track project can be edited repeatedly for at least several minutes without stopping playback, allocations/filesystem operations remain outside the callback, queued/active revision state is observable, and automated tests prove no duplicate/drop events around edit boundaries.