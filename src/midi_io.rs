#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MidiPortSelector {
    First,
    Index(usize),
    Name(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectDecision {
    Keep,
    Reconnect,
    Wait,
}

pub fn select_midi_port_index(
    names: &[String],
    selector: &MidiPortSelector,
) -> Result<usize, String> {
    if names.is_empty() {
        return Err("no MIDI input ports are available".into());
    }

    match selector {
        MidiPortSelector::First => Ok(0),
        MidiPortSelector::Index(index) if *index < names.len() => Ok(*index),
        MidiPortSelector::Index(index) => Err(format!(
            "MIDI input port index {index} is out of range ({} ports available)",
            names.len()
        )),
        MidiPortSelector::Name(requested) => {
            let requested = requested.trim();
            if requested.is_empty() {
                return Err("MIDI input port name may not be empty".into());
            }
            names
                .iter()
                .position(|name| name.eq_ignore_ascii_case(requested))
                .ok_or_else(|| format!("MIDI input port not found: {requested}"))
        }
    }
}

pub fn reconnect_decision(connected: bool, target_available: bool) -> ReconnectDecision {
    match (connected, target_available) {
        (true, true) => ReconnectDecision::Keep,
        (false, true) => ReconnectDecision::Reconnect,
        _ => ReconnectDecision::Wait,
    }
}

#[cfg(feature = "midi")]
mod backend {
    use super::{select_midi_port_index, MidiPortSelector};
    use crate::{decode_message, MidiEvent};
    use crossbeam_channel::{bounded, Receiver};
    use midir::{Ignore, MidiInput, MidiInputConnection};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    pub struct MidiInputHandle {
        _connection: MidiInputConnection<()>,
        receiver: Receiver<MidiEvent>,
        dropped_events: Arc<AtomicU64>,
        malformed_messages: Arc<AtomicU64>,
        port_name: String,
    }

    impl MidiInputHandle {
        pub fn port_name(&self) -> &str {
            &self.port_name
        }

        pub fn try_recv(&self) -> Option<MidiEvent> {
            self.receiver.try_recv().ok()
        }

        pub fn dropped_events(&self) -> u64 {
            self.dropped_events.load(Ordering::Relaxed)
        }

        pub fn malformed_messages(&self) -> u64 {
            self.malformed_messages.load(Ordering::Relaxed)
        }
    }

    pub fn list_midi_input_names() -> Result<Vec<String>, String> {
        let input = MidiInput::new("shelloop-enumerate")
            .map_err(|error| format!("failed to initialize MIDI input: {error}"))?;
        let ports = input.ports();
        ports
            .iter()
            .map(|port| {
                input
                    .port_name(port)
                    .map_err(|error| format!("failed to read MIDI input port name: {error}"))
            })
            .collect()
    }

    pub fn connect_midi_input(
        selector: &MidiPortSelector,
        queue_capacity: usize,
    ) -> Result<MidiInputHandle, String> {
        if queue_capacity == 0 {
            return Err("MIDI queue capacity must be greater than zero".into());
        }

        let mut input = MidiInput::new("shelloop-input")
            .map_err(|error| format!("failed to initialize MIDI input: {error}"))?;
        input.ignore(Ignore::None);

        let ports = input.ports();
        let names: Vec<String> = ports
            .iter()
            .map(|port| {
                input
                    .port_name(port)
                    .map_err(|error| format!("failed to read MIDI input port name: {error}"))
            })
            .collect::<Result<_, _>>()?;
        let index = select_midi_port_index(&names, selector)?;
        let port_name = names[index].clone();

        let (sender, receiver) = bounded(queue_capacity);
        let dropped_events = Arc::new(AtomicU64::new(0));
        let malformed_messages = Arc::new(AtomicU64::new(0));
        let dropped_for_callback = Arc::clone(&dropped_events);
        let malformed_for_callback = Arc::clone(&malformed_messages);

        let connection = input
            .connect(
                &ports[index],
                "shelloop-input-connection",
                move |_timestamp, message, _| match decode_message(message) {
                    Ok(event) => {
                        if sender.try_send(event).is_err() {
                            dropped_for_callback.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(_) => {
                        malformed_for_callback.fetch_add(1, Ordering::Relaxed);
                    }
                },
                (),
            )
            .map_err(|error| format!("failed to connect MIDI input {port_name}: {error}"))?;

        Ok(MidiInputHandle {
            _connection: connection,
            receiver,
            dropped_events,
            malformed_messages,
            port_name,
        })
    }
}

#[cfg(feature = "midi")]
pub use backend::{connect_midi_input, list_midi_input_names, MidiInputHandle};
