# Spec 12 — Invariant-Locked Variation

## Objective

Allow performers to generate controlled pattern variations while protecting chosen musical traits. The performer can lock exact anchor steps, overall rhythm, or melodic contour; Shelloop changes eligible fields elsewhere, previews the result, and applies it at a pattern boundary only after acceptance.

This is a deterministic variation tool, not a black-box generator. A candidate derives from the current pattern, invariant profile, mutation settings, algorithm version, and explicit seed.

## User outcomes

- Mark material that must survive a variation.
- Request reproducible variations with a seed and strength.
- Inspect changed steps/fields and why candidates were rejected.
- Audition/accept or reject a proposal without interrupting the current pattern.
- Undo/redo an accepted variation through existing editor history.

## Supported pattern fields

V1 may mutate active/inactive state, MIDI note, velocity, gate, probability, ratchet count, microtiming in frames, and swing. Enforce existing Pattern bounds, including notes 0-127 and existing timing/ratchet limits. No sound-engine feature is required.

## Invariants

Add an optional invariants object to Pattern JSON. Omission means no locks. Existing pattern and multitrack-project JSON must continue to load. Serialize the field only when nonempty. The profile has a schema version and validates before use.

V1 invariant kinds:

| Kind | Preserved property |
|---|---|
| anchor_steps | Exact full values, including event absence, at listed one-based step positions |
| activity_mask | Active/inactive state at every step; other fields may change |
| pitch_contour | Ordered semitone-interval sequence among active notes; preserve event/rest positions |
| event_count | Exact active event count across pattern |
| note_range | Each active note remains within inclusive MIDI bounds; a constraint, not an identity lock |

Each rule has a stable ID, enabled flag, kind, and parameters. Duplicate IDs, invalid indices/ranges, unknown kinds, unsupported versions, and contradictions fail before variation. Combined invariants are all mandatory. Reports identify conflicting invariant IDs.

### Persisted form

~~~json
{
  "name": "Example",
  "seed": 42,
  "swing": 0.08,
  "channel": 1,
  "steps": [],
  "invariants": {
    "version": 1,
    "rules": [
      {"id": "hook", "kind": "anchor_steps", "enabled": true, "steps": [1, 5, 9, 13]},
      {"id": "rhythm", "kind": "activity_mask", "enabled": true},
      {"id": "contour", "kind": "pitch_contour", "enabled": false}
    ]
  }
}
~~~

Represent invariants as control-thread metadata; CompiledPattern need not carry them. Add serde defaults so legacy files load. Verify whether the nested field can be added without changing the current project schema version; if not, add an explicit migration and preserve v1/v2 project loading.

## Commands

~~~text
variation lock anchors 1,5,9,13
variation lock rhythm on
variation lock contour on
variation lock note-range 24 60
variation locks
variation preview --seed 821 --amount 0.25
variation accept
variation reject
~~~

- Amount is finite [0,1] and targets the proportion of eligible steps receiving an attempt; it does not guarantee an exact changed-step count.
- Seed is required for reproducibility. If omitted, derive once from project seed, editor revision, and monotonic proposal counter; display and persist it in proposal metadata. Never use wall-clock entropy in the callback.
- One proposal is active per track. A new successful preview replaces the prior proposal.
- Preview does not alter editor draft, active compiled pattern, or history.
- Acceptance is one normal editor transaction and queues activation at the next pattern boundary by default.
- Reject/cancel removes only the proposal.
- Any source edit after preview invalidates it by revision mismatch.

## Deterministic mutation algorithm

1. Validate source pattern and invariant profile.
2. Build eligible step/field set from requested mutation scope and locks.
3. Initialize a documented stable PRNG from seed; do not use platform hash iteration order.
4. For a fixed attempt budget, choose target steps/fields and clone a candidate on the control thread.
5. Apply one bounded operation: toggle eligible activity, move eligible note, vary velocity/gate/probability, alter ratchets/microtiming/swing within limits.
6. Validate with Pattern::validate.
7. Evaluate enabled invariants in sorted rule-ID order.
8. On pass, produce proposal containing before/after hashes, changed fields, seed, attempt count, and each invariant result.
9. If no candidate passes before the cap, return “no valid variation found” and leave source untouched.

Selection and rule ordering must be stable across Windows/Linux for the same Shelloop version and seed. Version the mutation algorithm separately from invariant schema so future changes are explicit.

## Preview and acceptance

- Show seed, source revision, changed step/field list, and pass/fail for each invariant.
- If no candidate succeeds, identify locks that rejected attempts and suggest reducing amount or changing mutation scope.
- Audible preview uses a temporary compiled revision through the existing bounded quantized-change path. It cannot replace committed draft until acceptance.
- A/B uses at most two immutable compiled revisions; apply each at a pattern boundary and return to committed revision at a safe boundary.
- If audible preview cannot be implemented safely in an early increment, ship textual diff preview first and label it accurately.
- Accept updates draft/revision, clears redo per current editor semantics, and is undoable.
- If the pattern-change queue is full, acceptance fails visibly and leaves draft unchanged. Do not commit draft before queue acceptance.

## Thread and memory constraints

Invariant metadata, candidates, diff formatting, PRNG, and profile parsing are control-thread only. Audio sees only a compiled candidate at a quantized boundary. No RNG, heap allocation, serde, validation, or locks in the CPAL callback. Bound attempts, pattern length (256), and proposal storage (two drafts plus a small change list per track). Editor commit and queued revision must be atomic from the user's perspective.

## Failure behavior

- Invalid profile/source/seed/amount: clear error, no pattern change.
- Contradictory/impossible invariant set: no candidate and no editor change.
- Stale preview: reject acceptance and request new preview.
- Queue full: preserve source and proposal for retry.
- Reject: preserve source and undo/redo depth.
- Save/load preserves locks; legacy files use none.
- Unknown invariant kind/version rejects load with file/rule context; never silently discard locks.

## Tests

- Legacy pattern/project JSON without invariants loads and roundtrips.
- Each invariant passes matching candidates and rejects targeted violations.
- Combined locks all pass; deterministic diagnostics identify failures.
- Identical pattern/profile/seed/algorithm yields identical proposal bytes and changed-field list.
- Windows/Linux fixtures produce identical variation hash.
- Amount zero changes nothing; amount one remains bounded and obeys locks.
- Impossible profiles terminate at attempt cap and preserve pattern/history.
- Preview does not change draft, history, or active sound; stale revision invalidates it.
- Queue failure leaves editor, active revision, and history unchanged.
- Acceptance activates at pattern boundary; undo restores prior pattern and locks.
- Empty invariant metadata preserves current scheduling/audio output.
- Fuzz malformed profiles, indices, numeric boundaries, and duplicate IDs.

## Acceptance criteria

1. Existing files load without modification.
2. Every invariant validates and is checked before acceptance.
3. Same input/profile/seed/version produces same proposal across supported platforms.
4. Preview is non-destructive; acceptance is one undoable transaction.
5. Application is quantized and uses bounded queues.
6. Callback gains no parsing, allocation, RNG, locks, or blocking work.
7. UI distinguishes text preview from audible preview.
8. Tests pass in default/all-feature builds and strict Clippy passes Windows/Linux.

## Implementation slices

1. Define schema/defaults/validation and tests.
2. Implement pure invariant evaluation.
3. Implement stable seeded bounded candidate generation.
4. Implement proposal/revision lifecycle without audio changes.
5. Add typed editor commands and textual preview/accept/reject.
6. Add safe quantized compiled preview, A/B and atomic acceptance.
7. Add persistence compatibility, docs, examples and cross-platform tests.
