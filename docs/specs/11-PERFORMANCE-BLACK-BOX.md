# Spec 11 — Performance Black Box

## Objective

Provide opt-in retrospective capture for a live Shelloop performance. The performer can save recent audio and control/event history after an unexpected or especially good moment, without having pressed record beforehand.

The feature is a bounded rolling history, not an indefinite recorder. Audio is the source of truth for sound; the event sidecar makes a take inspectable and reproducible where inputs and engine state were captured.

## User outcomes

- Enable a defined rolling capture window before a session.
- Save the most recent complete window to a protected WAV plus machine-readable event sidecar.
- Continue performing while the save runs.
- See whether the captured interval contains missing audio/event blocks.
- Use the artifact to replay or investigate performance state later.

## Non-goals for v1

No unlimited recording or upload, microphone/input capture, external MIDI audio capture, guaranteed re-render of external hardware, or recording before opt-in. No disk, console, or lock operation belongs in the CPAL callback.

## User interface

### Startup options

~~~text
--black-box-seconds <SECONDS>  Arm retrospective capture for 1-120 seconds
--black-box-directory <DIR>    Directory used by quick-save
~~~

Capture is disabled by default. Supplying seconds arms capture; omission allocates no history buffer. Default quick-save directory is ./shelloop-takes and is not created until the user saves.

The requested history is bounded by a hard memory budget computed from negotiated sample rate, channels, and sample format. Startup reports requested and effective retention. If the minimum cannot fit, startup fails before opening audio and reports required and available bytes.

### In-session controls

- F8 saves the latest complete available window to the configured directory.
- The command prompt accepts blackbox status, blackbox save PATH, and blackbox clear.
- Status reports armed state, effective duration, oldest/newest frame, dropped audio blocks, dropped control events, and pending save state.
- Clear empties retained history on the collector thread; it does not delete files.
- Quick-save names use local wall-clock time plus a collision-resistant session suffix. Explicit paths protect existing files unless a later overwrite option is explicitly supplied.
- Save is asynchronous. The UI reports pending, success, or failure without waiting for disk.

When disabled, F8 explains the startup option needed; existing keys retain their behavior.

## Captured content

### Audio

- Tap final protected stereo output sent to the selected device, before device-format conversion when the tap sees float stereo master data.
- Store float32 samples at the actual negotiated sample rate.
- Save the newest completed frames available when the request is acknowledged.
- Preserve actual silence/clipping; do not normalize or apply another limiter.
- Write IEEE float WAV using the existing hound dependency.

### Event sidecar

Write UTF-8 JSON with the WAV basename and .json extension. Include schema/engine version, session ID, sample rate/channels, requested/effective duration, absolute frame interval, project/pattern content hash, tempo/grid/known seed, normalized input/control events, pattern revisions queued and activated, gaps/queue overflows, completeness status, save-request frame, and file hashes after successful write.

Do not store secrets, device system paths/serials, or arbitrary text outside Shelloop commands. User-readable device names may be included if already exposed. Distinguish captured facts from unavailable state; do not promise deterministic acoustic replay of external MIDI hardware.

## Event model

Each bounded, versioned control event contains monotonic sequence, absolute transport frame, kind (keyboard/MIDI note or CC, XY mix, transport, panic, editor command, revision queued/active, tempo change, capture command), validated fields needed to explain/replay the state transition, and source category (keyboard, MIDI, mouse, CLI, internal). Malformed events are rejected before enqueue and counted without affecting audio.

## Architecture and thread boundaries

1. CPAL callback taps the rendered protected stereo master.
2. It submits fixed-size blocks to a bounded preallocated nonblocking capture bridge. Queue failure drops capture only; playback never waits.
3. A dedicated collector thread owns rolling audio/event rings, indices, snapshots, WAV/JSON encoding, directory creation, hashing, and file replacement.
4. Control/UI/MIDI threads submit bounded records with frame timestamps. Callback-side enqueue is nonblocking.
5. Collector indexes audio by absolute frame. It verifies continuity and records missing ranges instead of filling silence invisibly.
6. Save carries an endpoint frame and request ID. Collector snapshots history ending at or before that endpoint and replies asynchronously.
7. WAV and JSON write to temporary sibling files. Finalize both before promotion; publish the sidecar last as a completion marker. On failure, remove temporary artifacts and report partial-commit state if the filesystem cannot atomically promote both files.

All buffers, messages, and save requests have fixed limits. Callback code uses preallocated memory and bounded nonblocking operations only. It must not create strings/vectors, lock, perform IO, hash, or allocate event objects.

## Bounds

- Requested retention: 1-120 seconds.
- Maximum retained audio memory: 128 MiB by default, including block overhead.
- Stereo float32 uses 8 bytes/frame; effective frames are min(requested × actual sample rate, memory budget / bytes per frame).
- Queue capacity covers a documented short collector stall but stays bounded. Queue-full increments counters and continues playback.
- Event history is bounded by time and count. At event-cap, drop oldest entries and record the affected range/count.
- Use sample frames for ordering/duration; wall time is metadata only.
- Sample-rate changes start a new capture segment. Never combine rates in one WAV.

## Failure behavior

- Disabled/unavailable: playback runs normally; status/save explains why.
- Queue full: continue playback, count missing frames/events, mark saved interval incomplete.
- Write/finalize failure: preserve existing targets, remove temporaries, report error, keep history for another save.
- Invalid path, permissions, disk full, or collision: report without stopping audio.
- Collector panic/disconnect: disable capture, preserve playback, report once and expose failure in status.
- Shutdown requests orderly finalization; only the control thread may wait, for a bounded interval, after audio stops.

## Compatibility

Existing --record remains forward-only and unchanged. Both systems may run together with independent queues/drop counts. Existing pattern/project schemas are unchanged. Existing keyboard, MIDI, mouse XY, panic, editor, and transport controls remain available. Help and docs distinguish forward recording from retrospective capture.

## Tests

### Logic and persistence

- Ring wraparound retains exactly the newest configured frames and complete interleaved frames.
- Save interval, short session and endpoint clamping are correct.
- Audio/event discontinuities expose missing ranges and never invent data.
- Equal-frame events sort by sequence.
- Disabled mode starts no worker, allocates no ring, and creates no directory/file.
- Invalid bounds and memory-budget failure reject before audio startup.
- Concurrent saves obey a fixed queue/path policy.
- Existing files are protected; write/hash failures clean temporary files and retain history.
- Full-queue shutdown does not deadlock.

### Runtime

- Mock callback submits while collector is stalled; render completes and counters rise.
- Arbitrary callback sizes and wrap boundaries yield correct frame totals.
- Forward recording and black-box capture coexist independently.
- Linux/Windows CI covers core logic/all-feature compilation. Hardware QA verifies saved master matches output and live load does not cause dropouts; CI cannot claim physical behavior.

## Acceptance criteria

1. Opt-in, disabled by default.
2. F8/command save captures latest available interval after request.
3. Audio and event files declare a common frame range, sample rate, and completeness.
4. Drops are visible and never presented as clean contiguous audio.
5. Callback work is bounded, allocation-free, and nonblocking.
6. Save failures preserve targets and playback.
7. Existing forward recording is unchanged with feature disabled.
8. Default/all-feature tests, Windows/Linux CI and strict Clippy pass.
9. Hardware verification is reported separately.

## Implementation slices

1. Define block/event/sidecar schemas with unit tests.
2. Build collector-owned bounded ring and snapshot tests, independent of CPAL.
3. Add callback bridge and overflow behavior.
4. Tap post-protection stereo master and capture control events.
5. Add CLI configuration, F8/status/save controls, and async feedback.
6. Add protected persistence, hashes, documentation, regression tests.
7. Run cross-platform CI and record hardware validation status.
