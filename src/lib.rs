pub mod audio;
pub mod command;
pub mod midi;
pub mod midi_io;
pub mod mixer;
pub mod pattern;
pub mod performance;
pub mod project;
pub mod quantize;
pub mod recording;
pub mod scheduler;
pub mod synth;
pub mod transport;
pub mod voice;

#[cfg(feature = "realtime-audio")]
pub use audio::{list_output_device_names, open_output_stream, AudioOutput};
pub use audio::{sanitize_sample, select_named_device_index, write_mono_interleaved};
pub use command::{parse_command, Command};
pub use midi::{decode_message, MidiEvent, MidiPerformanceState};
#[cfg(feature = "midi")]
pub use midi_io::{connect_midi_input, list_midi_input_names, MidiInputHandle};
pub use midi_io::{
    reconnect_decision, select_midi_port_index, MidiPortSelector, ReconnectDecision,
};
pub use mixer::{protect_master, ChannelStrip};
pub use pattern::{Pattern, PatternEvent, PatternScheduler, PatternStep};
pub use performance::{AxisCurve, AxisMapping, XyPoint};
pub use project::{Project, Track};
pub use quantize::{next_boundary_frame, QuantizeBoundary, QuantizedChange};
pub use recording::RecordingQueue;
pub use scheduler::{ScheduledEvent, Scheduler, StepEvent};
pub use synth::{Oscillator, SynthVoice};
pub use transport::{Transport, TransportState};
pub use voice::{VoiceAllocator, VoiceId, VoiceState};
