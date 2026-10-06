# Spec 10 — Internal Resampling

## Objective

Turn Shelloop's existing recording infrastructure into a creative resampling workflow: capture selected internal audio, finalize it safely off the callback, register it as a sample asset, and optionally place it onto a sample track without stopping the performance.

## User outcome

A performer can record a synth/effects phrase, stop capture at a musical boundary, and immediately trigger that result as a sample. This enables iterative sound design entirely inside Shelloop.

## Terminology

`Resampling` here means recording Shelloop's own rendered signal and reusing the captured audio as a new sample asset. It is distinct from sample-rate conversion, which is handled by sample playback.

## Capture sources

v1 supports:

- Master pre-protection or post-protection source; default post-protection for safety/consistency with existing recording.
- Individual track post-fader source.
- Send return source may be reserved for later.

Source selection is resolved to a stable route ID before capture begins.

## State machine

```rust
pub enum ResampleState {
    Idle,
    Armed { request: ResampleRequest },
    Recording { capture_id: CaptureId },
    Finalizing { capture_id: CaptureId },
    Ready { asset_id: SampleAssetId },
    Failed { message: String },
}
```

Audio callback owns only the bounded producer portion while recording. Finalization, WAV/header writing, asset decoding/registration and project mutation happen outside the callback.

## Request model

```rust
pub struct ResampleRequest {
    pub source: ResampleSource,
    pub start_boundary: QuantizeBoundary,
    pub stop: ResampleStop,
    pub normalize: bool,
    pub destination: ResampleDestination,
}

pub enum ResampleStop {
    Manual { boundary: QuantizeBoundary },
    Bars(u16),
    Seconds(f32),
}

pub enum ResampleDestination {
    AssetOnly,
    NewSampleTrack,
    ReplaceTrackAsset { track: TrackId },
}
```

## Quantized start/stop

Arming does not immediately write samples unless boundary is Immediate. At the exact resolved frame, callback begins capture. Manual stop similarly resolves to a future exact frame. If start/stop falls inside a CPAL buffer, capture begins/ends at the correct sample frame, not the callback boundary.

## Buffering

Reuse/generalize `RealtimeRecordingProducer` with stereo-frame support. Pool and channel capacities remain fixed. Queue overflow increments dropped-block counters. A resample with dropped blocks is marked degraded and is not silently treated as perfect.

## File and asset lifecycle

1. Capture writes to a temporary file under a project/session capture directory.
2. Background finalizer completes a valid WAV.
3. Optional normalization analyzes and rewrites/produces finalized audio outside real time.
4. Asset registrar validates the file and creates a `SampleAsset` for Spec 04.
5. Destination mutation is compiled and queued at a musical boundary.
6. Temporary files are retained or cleaned according to project policy.

Never make a partially written capture visible as a playable asset.

## Naming

Automatic names are deterministic/user-readable such as `Resample-0001.wav`; collision handling increments the counter. Project persistence stores the final relative asset path and ID.

## Normalization

Optional normalization is offline/background only. v1 peak normalization target may be configurable, default -1 dBFS. Do not normalize if the recording is silent below a defined threshold; report that condition instead.

## Destination behavior

`AssetOnly`: add asset to project library without routing it.

`NewSampleTrack`: allocate a new stable TrackId on the control thread, create a sample track referencing the asset, then submit a project snapshot replacement. It must respect max-track limit.

`ReplaceTrackAsset`: validate target is a sample track; replace its configured asset on chosen quantized boundary.

## Feedback prevention

Resampling the master and then immediately playing the resulting asset is allowed after capture completes. During active capture, routing must not automatically feed the same capture buffer back as a new source. No asset becomes playable until finalization.

## Failure behavior

- Disk write failure stops capture and reports failure without stopping audio playback where possible.
- Dropped recording blocks are reported in capture summary.
- Asset-registration failure leaves the finalized WAV available for recovery but does not mutate active project state.
- Track-limit or destination failure leaves asset in `AssetOnly` ready state rather than discarding audio.
- Shutdown while finalizing performs bounded/clean finalization according to existing recording shutdown policy.

## Commands/TUI

Examples:

```text
resample arm master bars 4
resample arm track 2 bars 2
resample start
resample stop bar
resample cancel
resample normalize on
resample destination new-track
```

TUI shows Armed, Recording with elapsed bars/time, Finalizing, and Ready/Failed. Never show Ready before asset registration succeeds.

## Tests

Write failing tests first for:

- Quantized start begins at exact requested frame inside a simulated block.
- Quantized stop excludes samples after exact stop frame.
- Fixed-duration bars produce correct frame count at known BPM/sample rate.
- Producer queue remains bounded and reports drops.
- Finalized capture is valid stereo/mono according to selected format.
- Asset is not visible before finalization.
- Failed asset registration leaves active project unchanged.
- New-track destination respects 16-track maximum.
- Replace destination rejects synth track.
- Normalization produces expected peak and handles silence.
- Resampled asset can be loaded by the sample playback engine.
- Master resampling plus later playback does not create accidental live feedback path.

## Completion criteria

A user can arm a 1–N bar capture from master or a selected track, capture starts/stops sample-accurately, finalization never blocks audio, the resulting WAV is automatically registered as a sample asset, and it can be assigned/played without restarting Shelloop.