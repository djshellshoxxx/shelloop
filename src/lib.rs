pub mod app;
pub mod audio;
pub mod command;
pub mod engine;
pub mod keyboard;
pub mod midi;
pub mod midi_io;
pub mod mixer;
pub mod multitrack;
pub mod pattern;
pub mod pc_speaker;
pub mod performance;
pub mod project;
pub mod quantize;
pub mod recording;
pub mod runtime;
pub mod scheduler;
pub mod sequencer;
pub mod synth;
pub mod transport;
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
pub use engine::{midi_note_hz, EngineCommand, RealtimeSynth};
pub use keyboard::{map_performance_key, shift_octave, PerformanceKey};
pub use midi::{decode_message, MidiEvent, MidiPerformanceState};
#[cfg(feature = "midi")]
pub use midi_io::{connect_midi_input, list_midi_input_names, MidiInputHandle};
pub use midi_io::{
    reconnect_decision, select_midi_port_index, MidiPortSelector, ReconnectDecision,
};
pub use mixer::{protect_master, ChannelStrip};
pub use multitrack::{
    EngineProjectSnapshot, MultiTrackEngine, MultiTrackProject, TrackCommand, TrackDefinition,
    TrackId, TrackKind, MULTITRACK_PROJECT_SCHEMA_VERSION, MAX_REALTIME_TRACKS,
};
pub use pattern::{parse_pattern_json, Pattern, PatternEvent, PatternScheduler, PatternStep};
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
pub use scheduler::{ScheduledEvent, Scheduler, StepEvent};
pub use sequencer::LiveSequencer;
pub use synth::{Oscillator, SynthVoice};
pub use transport::{Transport, TransportState};
pub use voice::{VoiceAllocator, VoiceId, VoiceState};
