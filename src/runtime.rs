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
        list_output_device_names, map_performance_key, open_stereo_output_stream,
        parse_pattern_edit_command, parse_pattern_json, parse_synth_parameter_command,
        pc_speaker_backend, protect_master, shift_octave, spawn_realtime_recording,
        CompiledPatternRevision, CompiledSynthPatch, EngineCommand, LiveSequencer,
        MultiTrackEngine, MultiTrackProject, Oscillator, PerformanceKey, PerformanceMix,
        ProjectPatternEditors, QuantizedChange, RealtimeSynth, StartupOptions, SynthPatch, TrackId,
        WavRecordingConfig, XyPoint,
    };
    use crossbeam_channel::{bounded, Sender};
    use crossterm::event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
        KeyboardEnhancementFlags, MouseButton, MouseEventKind, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    };
    use crossterm::execute;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement};
    use std::collections::HashMap;
    use std::fs;
    use std::io::{self, Write};
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };
    use std::thread;
    use std::time::{Duration, Instant};

    const COMMAND_QUEUE_CAPACITY: usize = 256;
    const SEQUENCER_CONTROL_QUEUE_CAPACITY: usize = 32;
    const PERFORMANCE_QUEUE_CAPACITY: usize = 32;
    const PATTERN_CHANGE_QUEUE_CAPACITY: usize = 16;
    const PATTERN_CHANGES_PER_SAMPLE_LIMIT: usize = 2;
    const SYNTH_PATCH_QUEUE_CAPACITY: usize = 16;
    const SYNTH_PATCHES_PER_SAMPLE_LIMIT: usize = 2;
    const EDIT_HISTORY_CAPACITY: usize = 128;
    #[cfg(feature = "midi")]
    const MIDI_QUEUE_CAPACITY: usize = 256;
    const COMMANDS_PER_SAMPLE_LIMIT: usize = 64;
    const SEQUENCER_CONTROLS_PER_SAMPLE_LIMIT: usize = 8;
    const KEYBOARD_CHANNEL: u8 = 0;
    const RECORDING_QUEUE_CAPACITY: usize = 64;
    const RECORDING_BLOCK_FRAMES: usize = 1024;
    const KEYBOARD_VELOCITY: f32 = 0.8;
    const FALLBACK_INITIAL_HOLD: Duration = Duration::from_millis(650);
    const FALLBACK_REPEAT_GRACE: Duration = Duration::from_millis(180);

    #[derive(Debug, Clone, Copy)]
    enum SequencerControl {
        TogglePlay,
        Restart,
        Panic,
    }

    #[derive(Debug, Clone, Copy)]
    struct HeldNote {
        note: u8,
        release_deadline: Option<Instant>,
    }

    struct TerminalGuard {
        keyboard_enhancement: bool,
        release_events_supported: bool,
        mouse_capture: bool,
    }

    impl TerminalGuard {
        fn enable(mouse_capture: bool) -> Result<Self, String> {
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

            if mouse_capture {
                let mut stdout = io::stdout();
                if let Err(error) = execute!(stdout, EnableMouseCapture) {
                    if keyboard_enhancement {
                        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
                    }
                    let _ = disable_raw_mode();
                    return Err(format!("failed to enable mouse capture: {error}"));
                }
            }

            Ok(Self {
                keyboard_enhancement,
                release_events_supported: cfg!(windows) || keyboard_enhancement,
                mouse_capture,
            })
        }

        fn release_events_supported(&self) -> bool {
            self.release_events_supported
        }
    }

    impl Drop for TerminalGuard {
        fn drop(&mut self) {
            if self.mouse_capture {
                let mut stdout = io::stdout();
                let _ = execute!(stdout, DisableMouseCapture);
            }
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

    fn send_sequencer_control(
        sender: &Sender<SequencerControl>,
        control: SequencerControl,
    ) -> Result<(), String> {
        sender
            .send_timeout(control, Duration::from_millis(50))
            .map_err(|error| format!("sequencer control queue unavailable: {error}"))
    }

    fn request_panic(
        command_sender: &Sender<EngineCommand>,
        sequencer_sender: &Sender<SequencerControl>,
    ) {
        let _ = command_sender.send_timeout(EngineCommand::Panic, Duration::from_millis(20));
        let _ = sequencer_sender.send_timeout(SequencerControl::Panic, Duration::from_millis(20));
        thread::sleep(Duration::from_millis(5));
    }

    fn render_edit_prompt(buffer: &str) -> Result<(), String> {
        print!("\x1b[2K\r:{buffer}");
        io::stdout()
            .flush()
            .map_err(|error| format!("failed to render edit command prompt: {error}"))
    }

    fn queue_live_edit(
        line: &str,
        editors: &mut ProjectPatternEditors,
        current_frame: u64,
        sample_rate: u32,
        bpm: f64,
        steps_per_beat: u32,
        sender: &Sender<(TrackId, QuantizedChange<CompiledPatternRevision>)>,
    ) -> Result<String, String> {
        let command = parse_pattern_edit_command(line)?;
        let boundary = editors.default_quantize_boundary(&command);
        let outcome = editors.apply(command)?;
        if !outcome.changed {
            return Ok(format!("selected track {}", outcome.track.0));
        }

        let (track, change) = editors.queue_selected_revision(
            current_frame,
            sample_rate,
            bpm,
            steps_per_beat,
            boundary,
        )?;
        sender
            .try_send((track, change))
            .map_err(|error| format!("pattern change queue unavailable: {error}"))?;

        Ok(format!(
            "queued track {} revision {} for frame {}",
            track.0, change.value.revision, change.apply_at_frame
        ))
    }

    fn queue_live_synth_edit(
        line: &str,
        track: TrackId,
        current: &mut SynthPatch,
        sample_rate: u32,
        sender: &Sender<(TrackId, CompiledSynthPatch)>,
    ) -> Result<String, String> {
        let (id, value) = parse_synth_parameter_command(line)?;
        let patch = current.with_parameter(id, value, sample_rate as f32)?;
        let compiled = CompiledSynthPatch::new(sample_rate as f32, patch)?;
        sender
            .try_send((track, compiled))
            .map_err(|error| format!("synth patch queue unavailable: {error}"))?;
        // Failed delivery leaves the control snapshot unchanged.
        *current = patch;
        Ok(format!(
            "queued synth parameter {id:?} for track {}",
            track.0
        ))
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

        println!("Special outputs:");
        println!("  PC speaker: {}", pc_speaker_backend().description());

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

        let pattern = match options.pattern_path.as_deref() {
            Some(path) => {
                let json = fs::read_to_string(path)
                    .map_err(|error| format!("failed to read pattern file {path:?}: {error}"))?;
                Some(parse_pattern_json(&json)?)
            }
            None => None,
        };
        let pattern_name = pattern.as_ref().map(|pattern| pattern.name.clone());

        let project =
            match options.project_path.as_deref() {
                Some(path) => Some(MultiTrackProject::load(path).map_err(|error| {
                    format!("failed to load multitrack project {path:?}: {error}")
                })?),
                None => None,
            };
        let project_track_count = project.as_ref().map(|project| project.tracks.len());
        let project_clock = project
            .as_ref()
            .map(|project| (project.bpm, u32::from(project.steps_per_beat)));
        let mut pattern_editors = project
            .as_ref()
            .map(|project| {
                ProjectPatternEditors::new(
                    project
                        .tracks
                        .iter()
                        .map(|track| (track.id, track.pattern.clone())),
                    EDIT_HISTORY_CAPACITY,
                )
            })
            .transpose()?;
        let mut synth_patches: HashMap<TrackId, SynthPatch> = project
            .as_ref()
            .map(|project| {
                project
                    .tracks
                    .iter()
                    .map(|track| {
                        (
                            track.id,
                            track
                                .synth_patch
                                .unwrap_or_else(|| SynthPatch::legacy(Oscillator::Saw)),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let has_sequencer = pattern.is_some() || project.is_some();

        let (command_sender, command_receiver) = bounded(COMMAND_QUEUE_CAPACITY);
        let (sequencer_sender, sequencer_receiver) = bounded(SEQUENCER_CONTROL_QUEUE_CAPACITY);
        let (performance_sender, performance_receiver) = bounded(PERFORMANCE_QUEUE_CAPACITY);
        let (pattern_change_sender, pattern_change_receiver) =
            bounded::<(TrackId, QuantizedChange<CompiledPatternRevision>)>(
                PATTERN_CHANGE_QUEUE_CAPACITY,
            );
        let (synth_patch_sender, synth_patch_receiver) =
            bounded::<(TrackId, CompiledSynthPatch)>(SYNTH_PATCH_QUEUE_CAPACITY);
        let transport_frame = Arc::new(AtomicU64::new(0));
        let audio_transport_frame = Arc::clone(&transport_frame);
        let polyphony = options.polyphony;
        let bpm = options.bpm;
        let steps_per_beat = options.steps_per_beat;
        let record_path = options.record_path.clone();
        let (recording_finalizer_sender, recording_finalizer_receiver) =
            std::sync::mpsc::sync_channel(1);
        let audio =
            open_stereo_output_stream(options.audio_device.as_deref(), move |sample_rate| {
                let mut recorder = match record_path.as_deref() {
                    Some(path) => {
                        let config =
                            WavRecordingConfig::new(sample_rate, 1, RECORDING_QUEUE_CAPACITY)?;
                        let (producer, finalizer) =
                            spawn_realtime_recording(path, config, RECORDING_BLOCK_FRAMES)?;
                        recording_finalizer_sender
                            .send(Some(finalizer))
                            .map_err(|_| "failed to publish recording finalizer".to_string())?;
                        Some(producer)
                    }
                    None => {
                        recording_finalizer_sender
                            .send(None)
                            .map_err(|_| "failed to publish recording state".to_string())?;
                        None
                    }
                };
                let mut performance_synth =
                    RealtimeSynth::new(sample_rate as f32, Oscillator::Saw, polyphony)?;
                let mut multitrack = match project {
                    Some(project) => Some(MultiTrackEngine::new(
                        sample_rate,
                        project.bpm,
                        u32::from(project.steps_per_beat),
                        project.seed,
                        polyphony,
                        project.tracks,
                    )?),
                    None => None,
                };
                let mut sequenced = match pattern {
                    Some(pattern) => Some((
                        LiveSequencer::new(
                            sample_rate,
                            f64::from(bpm),
                            u32::from(steps_per_beat),
                            pattern.seed,
                            pattern,
                        )?,
                        RealtimeSynth::new(sample_rate as f32, Oscillator::Saw, polyphony)?,
                    )),
                    None => None,
                };
                let mut sequencer_commands =
                    Vec::with_capacity(LiveSequencer::MAX_COMMANDS_PER_FRAME);
                let mut performance_mix = PerformanceMix::UNITY;

                Ok(move || {
                    while let Ok(next_mix) = performance_receiver.try_recv() {
                        performance_mix = next_mix;
                    }
                    for _ in 0..COMMANDS_PER_SAMPLE_LIMIT {
                        let Ok(command) = command_receiver.try_recv() else {
                            break;
                        };
                        performance_synth.handle(command);
                    }

                    for _ in 0..SYNTH_PATCHES_PER_SAMPLE_LIMIT {
                        let Ok((track, patch)) = synth_patch_receiver.try_recv() else {
                            break;
                        };
                        if let Some(engine) = multitrack.as_mut() {
                            engine.apply_synth_patch(track, patch);
                        }
                    }

                    for _ in 0..PATTERN_CHANGES_PER_SAMPLE_LIMIT {
                        let Ok((track, change)) = pattern_change_receiver.try_recv() else {
                            break;
                        };
                        if let Some(engine) = multitrack.as_mut() {
                            let _ = engine.queue_pattern_revision(track, change);
                        }
                    }

                    for _ in 0..SEQUENCER_CONTROLS_PER_SAMPLE_LIMIT {
                        let Ok(control) = sequencer_receiver.try_recv() else {
                            break;
                        };
                        if let Some(engine) = multitrack.as_mut() {
                            match control {
                                SequencerControl::TogglePlay => {
                                    engine.toggle_playing_all();
                                }
                                SequencerControl::Restart => engine.restart_all(),
                                SequencerControl::Panic => engine.panic_all(),
                            }
                        } else if let Some((sequencer, synth)) = sequenced.as_mut() {
                            match control {
                                SequencerControl::TogglePlay => {
                                    if sequencer.is_playing() {
                                        synth.handle(EngineCommand::Panic);
                                    }
                                    sequencer.toggle_playing();
                                }
                                SequencerControl::Restart => {
                                    synth.handle(EngineCommand::Panic);
                                    sequencer.restart();
                                }
                                SequencerControl::Panic => synth.handle(EngineCommand::Panic),
                            }
                        }
                    }

                    let (sequencer_left, sequencer_right) =
                        if let Some(engine) = multitrack.as_mut() {
                            let frame = engine.next_stereo_frame();
                            audio_transport_frame.store(engine.position_frame(), Ordering::Relaxed);
                            frame
                        } else if let Some((sequencer, synth)) = sequenced.as_mut() {
                            sequencer.fill_commands(&mut sequencer_commands);
                            for command in sequencer_commands.drain(..) {
                                synth.handle(command);
                            }
                            let sample = synth.next_sample();
                            (sample, sample)
                        } else {
                            (0.0, 0.0)
                        };

                    let live_sample = performance_synth.next_sample() * performance_mix.live_gain;
                    let left = protect_master(
                        live_sample + sequencer_left * performance_mix.sequencer_gain,
                    );
                    let right = protect_master(
                        live_sample + sequencer_right * performance_mix.sequencer_gain,
                    );
                    if let Some(recorder) = recorder.as_mut() {
                        let _ = recorder.push_sample(protect_master((left + right) * 0.5));
                    }
                    (left, right)
                })
            })?;
        let recording_finalizer = recording_finalizer_receiver
            .recv()
            .map_err(|_| "audio renderer did not publish recording state".to_string())?;

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
        if let Some(path) = options.record_path.as_deref() {
            println!("Recording mono master output to: {path}");
        }
        if let Some(name) = pattern_name.as_deref() {
            println!(
                "Pattern: {name} at {} BPM, {} steps/beat",
                options.bpm, options.steps_per_beat
            );
            println!("Sequencer: Space play/pause, Backspace restart");
        }
        if let (Some(path), Some(track_count)) =
            (options.project_path.as_deref(), project_track_count)
        {
            println!("Project: {path} ({track_count} tracks)");
            println!("Sequencer: Space play/pause all tracks, Backspace restart all tracks");
            println!("Live editor: press : for track/step/length/swing/rotate/undo/redo commands");
        }
        println!("Keys: Z-M/Q-U notes, [ ] octave, ! panic, ~ or Esc quit");
        if let Err(error) = io::stdout().flush() {
            request_panic(&command_sender, &sequencer_sender);
            drop(audio);
            if let Some(finalizer) = recording_finalizer {
                let _ = finalizer.finish();
            }
            return Err(format!("failed to flush terminal output: {error}"));
        }

        let terminal = match TerminalGuard::enable(options.mouse_xy) {
            Ok(terminal) => terminal,
            Err(error) => {
                request_panic(&command_sender, &sequencer_sender);
                drop(audio);
                if let Some(finalizer) = recording_finalizer {
                    let _ = finalizer.finish();
                }
                return Err(error);
            }
        };
        if options.mouse_xy {
            eprintln!("Mouse XY enabled: X crossfades live ↔ sequencer; Y controls overall level");
        }
        if !terminal.release_events_supported() {
            eprintln!(
                "terminal does not expose key-release events; keyboard notes use a timed fallback. \
                 MIDI input or a terminal supporting the kitty keyboard protocol gives better note gating"
            );
        }

        let mut octave = 0_i8;
        let mut held_notes = HashMap::<char, HeldNote>::new();
        let mut edit_command_buffer = None::<String>;
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
                let key_event = match terminal_event {
                    Event::Mouse(mouse_event) => {
                        if options.mouse_xy
                            && matches!(
                                mouse_event.kind,
                                MouseEventKind::Down(MouseButton::Left)
                                    | MouseEventKind::Drag(MouseButton::Left)
                            )
                        {
                            let (width, height) = crossterm::terminal::size().map_err(|error| {
                                format!("failed to query terminal size: {error}")
                            })?;
                            let point = XyPoint::from_terminal(
                                mouse_event.column,
                                mouse_event.row,
                                width,
                                height,
                            );
                            let mix = PerformanceMix::from_xy(point, has_sequencer);
                            let _ = performance_sender.try_send(mix);
                        }
                        continue;
                    }
                    Event::Key(key_event) => key_event,
                    _ => continue,
                };

                if let Some(mut buffer) = edit_command_buffer.take() {
                    if key_event.kind == KeyEventKind::Release {
                        edit_command_buffer = Some(buffer);
                        continue;
                    }

                    match key_event.code {
                        KeyCode::Esc => {
                            println!();
                        }
                        KeyCode::Enter => {
                            println!();
                            if let (Some(editors), Some((project_bpm, project_steps))) =
                                (pattern_editors.as_mut(), project_clock)
                            {
                                let current_frame = transport_frame.load(Ordering::Relaxed);
                                let result = if buffer.split_whitespace().next() == Some("synth") {
                                    let track = editors.selected_track();
                                    match synth_patches.get_mut(&track) {
                                        Some(current) => queue_live_synth_edit(
                                            &buffer,
                                            track,
                                            current,
                                            audio.sample_rate(),
                                            &synth_patch_sender,
                                        ),
                                        None => Err("selected track has no synth patch".into()),
                                    }
                                } else {
                                    queue_live_edit(
                                        &buffer,
                                        editors,
                                        current_frame,
                                        audio.sample_rate(),
                                        project_bpm,
                                        project_steps,
                                        &pattern_change_sender,
                                    )
                                };
                                match result {
                                    Ok(message) => println!("edit: {message}"),
                                    Err(error) => eprintln!("edit error: {error}"),
                                }
                            }
                        }
                        KeyCode::Backspace => {
                            buffer.pop();
                            render_edit_prompt(&buffer)?;
                            edit_command_buffer = Some(buffer);
                        }
                        KeyCode::Char(character) if !character.is_control() => {
                            buffer.push(character);
                            render_edit_prompt(&buffer)?;
                            edit_command_buffer = Some(buffer);
                        }
                        _ => {
                            edit_command_buffer = Some(buffer);
                        }
                    }
                    continue;
                }

                if key_event.code == KeyCode::Char(':')
                    && key_event.kind == KeyEventKind::Press
                    && pattern_editors.is_some()
                {
                    edit_command_buffer = Some(String::new());
                    render_edit_prompt("")?;
                    continue;
                }

                if key_event.code == KeyCode::Esc && key_event.kind != KeyEventKind::Release {
                    break 'session;
                }
                if key_event.code == KeyCode::Backspace
                    && key_event.kind == KeyEventKind::Press
                    && has_sequencer
                {
                    send_sequencer_control(&sequencer_sender, SequencerControl::Restart)?;
                    continue;
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
                            send_sequencer_control(&sequencer_sender, SequencerControl::Panic)?;
                            held_notes.clear();
                        }
                        Some(PerformanceKey::Quit) => break 'session,
                        Some(PerformanceKey::TogglePlay) if has_sequencer => {
                            send_sequencer_control(
                                &sequencer_sender,
                                SequencerControl::TogglePlay,
                            )?;
                        }
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

        request_panic(&command_sender, &sequencer_sender);
        drop(terminal);
        drop(audio);

        let recording_result = match recording_finalizer {
            Some(finalizer) => finalizer.finish().map(|summary| {
                if let Some(path) = options.record_path.as_deref() {
                    println!(
                        "Recording saved: {} frames to {} (dropped blocks: {}, rejected blocks: {})",
                        summary.frames_written,
                        path,
                        summary.dropped_blocks,
                        summary.rejected_blocks
                    );
                }
            }),
            None => Ok(()),
        };

        match (session_result, recording_result) {
            (Err(session_error), Err(recording_error)) => Err(format!(
                "{session_error}; recording finalization also failed: {recording_error}"
            )),
            (Err(session_error), Ok(())) => Err(session_error),
            (Ok(()), Err(recording_error)) => Err(recording_error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }
    #[cfg(test)]
    mod synth_control_tests {
        use super::*;

        #[test]
        fn failed_queue_delivery_preserves_control_patch() {
            let (sender, receiver) = bounded(1);
            let mut patch = SynthPatch::legacy(Oscillator::Saw);
            queue_live_synth_edit(
                "synth output_gain 0.4",
                TrackId(1),
                &mut patch,
                48_000,
                &sender,
            )
            .unwrap();
            let accepted = patch;
            assert!(queue_live_synth_edit(
                "synth output_gain 0.2",
                TrackId(1),
                &mut patch,
                48_000,
                &sender
            )
            .is_err());
            assert_eq!(patch, accepted);
            let (track, compiled) = receiver.try_recv().unwrap();
            assert_eq!(track, TrackId(1));
            let mut synth = RealtimeSynth::new(48_000.0, Oscillator::Saw, 4).unwrap();
            assert!(synth.apply_patch(compiled));
            assert_eq!(synth.patch(), accepted);
            drop(receiver);
            assert!(queue_live_synth_edit(
                "synth output_gain 0.2",
                TrackId(1),
                &mut patch,
                48_000,
                &sender
            )
            .is_err());
            assert_eq!(patch, accepted);
        }

        #[test]
        fn invalid_edit_never_enters_the_audio_queue() {
            let (sender, receiver) = bounded(1);
            let mut patch = SynthPatch::legacy(Oscillator::Saw);
            let original = patch;
            assert!(queue_live_synth_edit(
                "synth filter_cutoff 20000",
                TrackId(1),
                &mut patch,
                32_000,
                &sender
            )
            .is_err());
            assert_eq!(patch, original);
            assert!(receiver.try_recv().is_err());
        }
    }
}

#[cfg(all(feature = "realtime-audio", feature = "terminal-ui"))]
pub use live::run_realtime_session;
