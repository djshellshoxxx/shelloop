# Spec 04 — WAV Sample Playback

## Objective

Add bounded in-memory WAV sample instruments for drum hits, one-shots and loops without filesystem access in the audio callback.

## User outcome

A track can trigger kicks, snares, vocal hits, textures or loops from WAV files using the same deterministic pattern scheduler used by synth tracks.

## Supported input

Initial supported decode target: PCM or float WAV. At minimum support mono/stereo 16-bit PCM, 24-bit PCM, 32-bit PCM and 32-bit float where the decoder library safely supports them. Unsupported formats fail during load with a clear error.

Files are decoded and normalized into internal `f32` buffers before playback. File I/O and decoding never occur in the audio callback.

## Sample asset model

```rust
pub struct SampleAssetId(pub u32);

pub struct SampleAsset {
    pub id: SampleAssetId,
    pub source_path: PathBuf,       // control/persistence only
    pub source_sample_rate: u32,
    pub channels: u16,
    pub frames: Arc<[StereoFrame]>,
}
```

The real-time side receives only the ID, decoded immutable frame storage, and precompiled playback metadata.

## Playback modes

- `OneShot`: trigger plays until sample end; note-off does not stop it.
- `Gate`: playback continues while note is held and enters configured short release on note-off.
- `Loop`: loop between validated start/end frames while held/sequenced gate remains active.

v1 sample voice parameters:

- Asset ID.
- Start frame or normalized start position.
- End frame.
- Loop start/end where relevant.
- Gain.
- Pan.
- Pitch in semitones/cents.
- Reverse boolean.
- Attack/release micro-fades.

## Resampling

Source sample rate may differ from output sample rate. v1 requires a deterministic interpolation strategy. Linear interpolation is acceptable initially if isolated behind a resampler interface so higher-quality interpolation can be added without changing project contracts.

Playback increment:

`increment = source_rate / output_rate * 2^(pitch_semitones/12)`.

All index math is checked during compilation. No out-of-bounds access is allowed during reverse or loop playback.

## Polyphony

Each sample track has a fixed maximum sample voice count, default 16 and project-configurable within a bounded range. Voice stealing is deterministic: expired/released first, then oldest active.

## Asset loading lifecycle

1. Control thread resolves project path.
2. Decode/validate WAV.
3. Convert to internal immutable frame buffer.
4. Register asset in an `AssetBank` snapshot.
5. Quantized project/track replacement publishes the new asset reference.

Missing files do not crash a project. Project load reports all missing/invalid assets and either refuses activation or marks those tracks unavailable according to explicit load mode. Default is fail-before-activation.

## Pattern integration

Pattern steps need an instrument-independent trigger representation. Do not encode sample paths in steps. A sample track pattern triggers its configured sample slot/voice. Future drum-rack support can extend this with slot IDs.

## Memory limits

Define a configurable but bounded decoded sample memory budget. Default target: 512 MiB per project. Loading beyond the limit fails with a report of requested/current bytes. The callback never pages assets from disk.

## Tests

Write failing tests first for:

- WAV decoding converts supported formats to expected f32 frames.
- Mono is duplicated or represented consistently as stereo.
- Unsupported/corrupt WAV fails before activation.
- 44.1 kHz sample plays at correct duration on 48 kHz output within interpolation tolerance.
- +12 semitones approximately halves playback duration.
- Reverse starts/ends at correct frames without underflow.
- Loop boundaries do not double/drop a frame unexpectedly.
- One-shot ignores note-off; gate obeys note-off.
- Voice stealing is deterministic.
- Asset memory budget is enforced.
- Missing asset leaves current active project untouched.
- Rendering remains finite for maximum pitch/rate values.

## Completion criteria

A multi-track project can mix synth and sample tracks, load drum one-shots and loops before audio startup, transpose them, play them deterministically from patterns, and survive invalid/missing sample files without destabilizing the existing session.