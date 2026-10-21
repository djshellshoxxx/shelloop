# Spec 16 — Euclidean rhythm generator

Status: implemented 2026-10-21 (see implementation notes). Scope: pattern editing on the control thread only.

## Goal
Let a performer place an evenly distributed (Euclidean) rhythm on the selected track's pattern with one live command, using the same transactional, undoable, quantised edit path as other pattern edits.

## Command
```text
euclid <hits> [rotation]
```
* `hits` is an integer from 0 to the current pattern length. `hits = 0` clears the rhythm.
* `rotation` is an optional non-negative integer that rotates the result to the right (reusing the existing rotation convention). Default 0.
* The step count is the selected pattern's current length; there is no separate step argument.

## Algorithm
Bjorklund's algorithm (Toussaint, "The Euclidean Algorithm Generates Traditional Musical Rhythms"). For `hits = k`, `steps = n`, the result contains exactly `k` hits and is a rotation of the canonical E(k, n) sequence. `E(3, 8)` is `x..x..x.`, `E(5, 8)` is `x.xx.xx.`. Deterministic: no randomness.

## Editing semantics
* Steps selected by the rhythm that were already active keep their note, velocity, gate, probability, ratchets and microtiming.
* Steps selected that were empty take the editor's default step (note 60, velocity 0.8, gate 0.5, probability 1.0, ratchets 1, microtiming 0), matching `step N toggle`.
* Active steps not selected become empty.
* Parameter locks are not removed: a lock on an emptied step stays as a trigless lock (existing rule), and locks on kept steps are untouched.
* The edit is one undoable transaction (`undo` restores the previous pattern), and it is quantised to the pattern boundary like `rotate` and `length`.
* Invalid input (non-integer, `hits > length`, negative values) is rejected before any state changes.
* Pattern invariant profiles are not re-checked by this command, the same as manual step edits.

## Real-time contract
Control-thread only. The audio thread receives the usual compiled revision; nothing about the callback changes.

## Acceptance
* Pure-function tests: hit counts for every `k` in `0..=n` for n in 1..=32, canonical patterns E(3,8), E(5,8), E(2,5), rotation invariance.
* Editor tests (`tests/euclid.rs`): kept hits keep their note, new hits take the default step, one undo restores the previous pattern, and the edit is pattern-quantised. Controller test: `euclid` queues one revision that applies at the next boundary (frame 0 applies at frame 1).
* Errors: `euclid 9` on an 8-step pattern and `euclid -1` are rejected with no revision change.

## Implementation notes
* `src/euclid.rs` — `euclidean_mask(hits, steps, rotation)` (pure, no allocation outside the returned vector).
* `src/editor.rs` — `PatternEditor::apply_euclidean` builds one candidate pattern and commits it.
* `src/editor_command.rs` — `PatternEditCommand::Euclid { hits, rotation }`, parsed from `euclid`, quantised at the pattern boundary.
