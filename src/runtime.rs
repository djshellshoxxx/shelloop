use crate::{EngineCommand, MidiEvent};

pub fn engine_command_from_midi(event: MidiEvent) -> Option<EngineCommand> {
    match event {
        MidiEvent::NoteOn {
            channel,
            note,
            velocity,
        } => Some(EngineCommand::NoteOn {
            channel,
            note,
            velocity: f32::from(velocity) / 127.0,
        }),
        MidiEvent::NoteOff { channel, note, .. } => Some(EngineCommand::NoteOff { channel, note }),
        MidiEvent::ControlChange {
            channel,
            controller: 64,
            value,
        } => Some(EngineCommand::Sustain {
            channel,
            enabled: value >= 64,
        }),
        MidiEvent::ControlChange {
            controller: 120 | 123,
            ..
        } => Some(EngineCommand::Panic),
        MidiEvent::ControlChange { .. } | MidiEvent::PitchBend { .. } => None,
    }
}

#[cfg(all(feature = "realtime-audio", feature = "terminal-ui"))]
mod live {
    #[cfg(feature = "midi")]
    use super::engine_command_from_midi;
    #[cfg(feature = "midi")]
    use crate::{
        connect_midi_input, list_midi_input_names, select_midi_port_index, MidiPortSelector,
    };
    use crate::{
        list_output_device_names, map_performance_key, open_output_stream, shift_octave,
        EngineCommand, Oscillator, PerformanceKey, RealtimeSynth, StartupOptions,
    };
    use crossbeam_channel::{bounded, Sender};
    use crossterm::event::{
        self, Event, KeyCode, KeyEventKind, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    };
    use crossterm::execute;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement};
    use std::collections::HashMap;
    use std::io::{self, Write};
    use std::thread;
    use std::time::{Duration, Instant};

    const COMMAND_QUEUE_CAPACITY: usize = 256;
    #[cfg(feature = "midi")]
    const MIDI_QUEUE_CAPACITY: usize = 256;
    const COMMANDS_PER_SAMPLE_LIMIT: usize = 64;
    const KEYBOARD_CHANNEL: u8 = 0;
    const KEYBOARD_VELOCITY: f32 = 0.8;
    const FALLBACK_INITIAL_HOLD: Duration = Duration::from_millis(650);
    const FALLBACK_REPEAT_GRACE: Duration = Duration::from_millis(180);

    #[derive(Debug, Clone, Copy)]
    struct HeldNote {
        note: u8,
        release_deadline: Option<Instant>,
    }

    struct TerminalGuard {
        keyboard_enhancement: bool,
        release_events_supported: bool,
    }

    impl TerminalGuard {
        fn enable() -> Result<Self, String> {
            enable_raw_mode()
                .map_err(|error| format!("failed to enter terminal raw mode: {error}"))?;

            let keyboard_enhancement = supports_keyboard_enhancement().unwrap_or(false);
            if keyboard_enhancement {
                let flags = KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                    | KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
                let mut stdout = io::stdout();
                if let Err(error) = execute!(stdout, PushKeyboardEnhancementFlags(flags)) {
                    let _ = disable_raw_mode();
                    return Err(format!(
                        "terminal reported keyboard enhancement support but enabling it failed: {error}"
                    ));
                }
            }

            Ok(Self {
                keyboard_enhancement,
                release_events_supported: cfg!(windows) || keyboard_enhancement,
            })
        }

        fn release_events_supported(&self) -> bool {
            self.release_events_supported
        }
    }

    impl Drop for TerminalGuard {
        fn drop(&mut self) {
            if self.keyboard_enhancement {
                let mut stdout = io::stdout();
                let _ = execute!(stdout, PopKeyboardEnhancementFlags);
            }
            let _ = disable_raw_mode();
        }
    }

    fn send_command(sender: &Sender<EngineCommand>, command: EngineCommand) -> Result<(), String> {
        sender
            .send_timeout(command, Duration::from_millis(50))
            .map_err(|error| format!("audio command queue unavailable: {error}"))
    }

    fn request_panic(sender: &Sender<EngineCommand>) {
        let _ = sender.send_timeout(EngineCommand::Panic, Duration::from_millis(20));
        thread::sleep(Duration::from_millis(5));
    }

    fn print_devices() -> Result<(), String> {
        println!("Audio output devices:");
        let audio_devices = list_output_device_names()?;
        if audio_devices.is_empty() {
            println!("  (none)");
        } else {
            for (index, name) in audio_devices.iter().enumerate() {
                println!("  [{index}] {name}");
            }
        }

        println!("MIDI input devices:");
        #[cfg(feature = "midi")]
        {
            let midi_devices = list_midi_input_names()?;
            if midi_devices.is_empty() {
                println!("  (none)");
            } else {
                for (index, name) in midi_devices.iter().enumerate() {
                    println!("  [{index}] {name}");
                }
            }
        }
        #[cfg(not(feature = "midi"))]
        println!("  (MIDI support not compiled in)");

        Ok(())
    }

    pub fn run_realtime_session(options: StartupOptions) -> Result<(), String> {
        if options.list_devices {
            return print_devices();
        }

        #[cfg(not(feature = "midi"))]
        if !options.no_midi {
            return Err(
                "MIDI support is not compiled in; rebuild with feature `midi` or pass --no-midi"
                    .into(),
            );
        }

        let (command_sender, command_receiver) = bounded(COMMAND_QUEUE_CAPACITY);
        let polyphony = options.polyphony;
        let audio = open_output_stream(options.audio_device.as_deref(), move |sample_rate| {
            let mut synth = RealtimeSynth::new(sample_rate as f32, Oscillator::Saw, polyphony)?;
            Ok(move || {
                for _ in 0..COMMANDS_PER_SAMPLE_LIMIT {
                    let Ok(command) = command_receiver.try_recv() else {
                        break;
                    };
                    synth.handle(command);
                }
                synth.next_sample()
            })
        })?;

        #[cfg(feature = "midi")]
        let midi_selector = options.midi_port.clone().unwrap_or(MidiPortSelector::First);
        #[cfg(feature = "midi")]
        let mut midi = if options.no_midi {
            None
        } else {
            match connect_midi_input(&midi_selector, MIDI_QUEUE_CAPACITY) {
                Ok(handle) => Some(handle),
                Err(error) => {
                    eprintln!("MIDI input unavailable: {error}; keyboard control remains active");
                    None
                }
            }
        };

        println!(
            "shelloop running: audio=\"{}\" {} Hz / {} ch, polyphony={} voices",
            audio.device_name(),
            audio.sample_rate(),
            audio.channels(),
            options.polyphony
        );
        #[cfg(feature = "midi")]
        if let Some(handle) = midi.as_ref() {
            println!("MIDI input: {}", handle.port_name());
        }
        println!("Keys: Z-M/Q-U notes, [ ] octave, ! panic, ~ or Esc quit");
        if let Err(error) = io::stdout().flush() {
            request_panic(&command_sender);
            return Err(format!("failed to flush terminal output: {error}"));
        }

        let terminal = match TerminalGuard::enable() {
            Ok(terminal) => terminal,
            Err(error) => {
                request_panic(&command_sender);
                return Err(error);
            }
        };
        if !terminal.release_events_supported() {
            eprintln!(
                "terminal does not expose key-release events; keyboard notes use a timed fallback. \
                 MIDI input or a terminal supporting the kitty keyboard protocol gives better note gating"
            );
        }

        let mut octave = 0_i8;
        let mut held_notes = HashMap::<char, HeldNote>::new();
        #[cfg(feature = "midi")]
        let mut last_midi_scan = Instant::now();

        let session_result = (|| -> Result<(), String> {
            'session: loop {
                if let Some(error) = audio.take_error() {
                    return Err(format!("audio stream error: {error}"));
                }

                if !terminal.release_events_supported() {
                    let now = Instant::now();
                    let expired: Vec<char> = held_notes
                        .iter()
                        .filter_map(|(key, held)| {
                            held.release_deadline
                                .filter(|deadline| now >= *deadline)
                                .map(|_| *key)
                        })
                        .collect();
                    for key in expired {
                        if let Some(held) = held_notes.remove(&key) {
                            send_command(
                                &command_sender,
                                EngineCommand::NoteOff {
                                    channel: KEYBOARD_CHANNEL,
                                    note: held.note,
                                },
                            )?;
                        }
                    }
                }

                #[cfg(feature = "midi")]
                {
                    if let Some(handle) = midi.as_ref() {
                        while let Some(event) = handle.try_recv() {
                            if let Some(command) = engine_command_from_midi(event) {
                                send_command(&command_sender, command)?;
                            }
                        }
                    }

                    if !options.no_midi && last_midi_scan.elapsed() >= Duration::from_secs(1) {
                        last_midi_scan = Instant::now();
                        if let Ok(names) = list_midi_input_names() {
                            let desired_name = select_midi_port_index(&names, &midi_selector)
                                .ok()
                                .and_then(|index| names.get(index).cloned());
                            let current_name =
                                midi.as_ref().map(|handle| handle.port_name().to_string());

                            if current_name != desired_name {
                                if current_name.is_some() {
                                    midi = None;
                                    let _ = send_command(&command_sender, EngineCommand::Panic);
                                }
                                if desired_name.is_some() {
                                    if let Ok(handle) =
                                        connect_midi_input(&midi_selector, MIDI_QUEUE_CAPACITY)
                                    {
                                        eprintln!("MIDI input connected: {}", handle.port_name());
                                        midi = Some(handle);
                                    }
                                }
                            }
                        }
                    }
                }

                if !event::poll(Duration::from_millis(10))
                    .map_err(|error| format!("terminal event polling failed: {error}"))?
                {
                    continue;
                }

                let terminal_event = event::read()
                    .map_err(|error| format!("failed to read terminal event: {error}"))?;
                let Event::Key(key_event) = terminal_event else {
                    continue;
                };

                if key_event.code == KeyCode::Esc && key_event.kind != KeyEventKind::Release {
                    break 'session;
                }

                let KeyCode::Char(character) = key_event.code else {
                    continue;
                };
                let held_key = character.to_ascii_lowercase();

                match key_event.kind {
                    KeyEventKind::Press => match map_performance_key(character, octave) {
                        Some(PerformanceKey::Note(note)) => {
                            if let Some(held) = held_notes.get_mut(&held_key) {
                                if !terminal.release_events_supported() {
                                    held.release_deadline =
                                        Some(Instant::now() + FALLBACK_REPEAT_GRACE);
                                }
                                continue;
                            }
                            send_command(
                                &command_sender,
                                EngineCommand::NoteOn {
                                    channel: KEYBOARD_CHANNEL,
                                    note,
                                    velocity: KEYBOARD_VELOCITY,
                                },
                            )?;
                            let release_deadline = (!terminal.release_events_supported())
                                .then_some(Instant::now() + FALLBACK_INITIAL_HOLD);
                            held_notes.insert(
                                held_key,
                                HeldNote {
                                    note,
                                    release_deadline,
                                },
                            );
                        }
                        Some(PerformanceKey::OctaveDown) => octave = shift_octave(octave, -1),
                        Some(PerformanceKey::OctaveUp) => octave = shift_octave(octave, 1),
                        Some(PerformanceKey::Panic) => {
                            send_command(&command_sender, EngineCommand::Panic)?;
                            held_notes.clear();
                        }
                        Some(PerformanceKey::Quit) => break 'session,
                        Some(PerformanceKey::TogglePlay) | None => {}
                    },
                    KeyEventKind::Release => {
                        if let Some(held) = held_notes.remove(&held_key) {
                            send_command(
                                &command_sender,
                                EngineCommand::NoteOff {
                                    channel: KEYBOARD_CHANNEL,
                                    note: held.note,
                                },
                            )?;
                        }
                    }
                    KeyEventKind::Repeat => {
                        if !terminal.release_events_supported() {
                            if let Some(held) = held_notes.get_mut(&held_key) {
                                held.release_deadline =
                                    Some(Instant::now() + FALLBACK_REPEAT_GRACE);
                            }
                        }
                    }
                }
            }
            Ok(())
        })();

        request_panic(&command_sender);
        drop(terminal);
        session_result
    }
}

#[cfg(all(feature = "realtime-audio", feature = "terminal-ui"))]
pub use live::run_realtime_session;
