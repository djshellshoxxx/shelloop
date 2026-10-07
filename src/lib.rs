pub mod app;
pub mod audio;
pub mod blackbox;
pub mod command;
pub mod editor;
pub mod editor_command;
pub mod effects;
pub mod engine;
pub mod envelope;
pub mod filter;
pub mod keyboard;
pub mod midi;
pub mod midi_io;
pub mod mixer;
pub mod multitrack;
pub mod params;
pub mod pattern;
pub mod pc_speaker;
pub mod performance;
pub mod project;
pub mod quantize;
pub mod recording;
pub mod runtime;
pub mod sample;
pub mod scheduler;
pub mod scope;
pub mod sequencer;
pub mod synth;
pub mod transport;
pub mod variation;
pub mod voice;

pub use app::{parse_startup_options, StartupOptions};
pub use audio::{
    create_sample_renderer, sanitize_sample, select_named_device_index, write_mono_interleaved,
    write_stereo_interleaved,
};
#[cfg(feature = "realtime-audio")]
pub use audio::{
    list_output_device_names, open_output_stream, open_stereo_output_stream, AudioOutput,
};
pub use command::{parse_command, Command};
pub use editor::{
    compile_pattern_revision, CompiledPatternRevision, PatternEditor, PatternRevision,
};
pub use editor_command::{
    parse_pattern_edit_command, EditOutcome, PatternEditCommand, ProjectPatternEditors, StepEdit,
};
pub use engine::{midi_note_hz, EngineCommand, RealtimeSynth};
pub use envelope::{AdsrEnvelope, AdsrParams, EnvelopeStage};
pub use filter::{clamp_cutoff, max_cutoff, FilterMode, FilterParams, StateVariableFilter};
pub use keyboard::{map_performance_key, shift_octave, PerformanceKey};
pub use midi::{decode_message, MidiEvent, MidiPerformanceState};
#[cfg(feature = "midi")]
pub use midi_io::{connect_midi_input, list_midi_input_names, MidiInputHandle};
pub use midi_io::{
    reconnect_decision, select_midi_port_index, MidiPortSelector, ReconnectDecision,
};
pub use mixer::{protect_master, ChannelStrip, SmoothedParam};
pub use multitrack::{
    EngineProjectSnapshot, MultiTrackEngine, MultiTrackProject, SampleContext, TrackCommand,
    TrackDefinition, TrackId, TrackKind, MAX_REALTIME_TRACKS, MULTITRACK_PROJECT_SCHEMA_VERSION,
};
pub use pattern::{
    parse_pattern_json, CompiledPattern, LockBoundary, LockTarget, ParameterLock, Pattern,
    PatternEvent, PatternScheduler, PatternStep, StepLocks, MAX_LOCKS_PER_STEP, MAX_PATTERN_LOCKS,
    MAX_PATTERN_STEPS,
};
pub use pc_speaker::{pc_speaker_backend, pc_speaker_test, PcSpeakerBackend};
pub use performance::{AxisCurve, AxisMapping, PerformanceMix, XyPoint};
pub use project::{Project, Track};
pub use quantize::{next_boundary_frame, QuantizeBoundary, QuantizedChange};
pub use recording::{
    spawn_realtime_recording, RealtimeRecordingBridge, RealtimeRecordingFinalizer,
    RealtimeRecordingProducer, RecordingQueue, RecordingSummary, RecordingWriter,
    WavRecordingConfig,
};
pub use runtime::engine_command_from_midi;
#[cfg(all(feature = "realtime-audio", feature = "terminal-ui"))]
pub use runtime::run_realtime_session;
pub use sample::{
    decode_wav_file, decode_wav_reader, CompiledSamplePlayback, DecodedWav, RealtimeSampler,
    SampleAsset, SampleAssetBank, SampleAssetId, SampleMode, SampleSettings, StereoFrame,
    DEFAULT_SAMPLE_MEMORY_BUDGET,
};
pub use scheduler::{ScheduledEvent, Scheduler, StepEvent};
pub use scope::{PanelLayout, PeakHistory, ScopeTap, WaveformStyle, SCOPE_CAPACITY};
pub use sequencer::LiveSequencer;
pub use synth::{
    parse_synth_parameter_command, CompiledSynthPatch, Oscillator, SynthParamId, SynthParamValue,
    SynthPatch, SynthVoice,
};
pub use transport::{Transport, TransportState};
pub use voice::{VoiceAllocator, VoiceId, VoiceState};
pub use params::{
    ActionId, EffectLocation, EffectParamId, EffectSlotId, GlobalParamId, ParamCurve,
    ParamDescriptor, ParameterTarget, SampleParamId, TrackParamId,
};
pub use variation::{InvariantKind, InvariantProfile, InvariantRule};
