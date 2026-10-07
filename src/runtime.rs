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
    use crate::blackbox::{
        new_black_box_session_id, spawn_black_box, BlackBoxConfig, BlackBoxHandle, BlackBoxStatus,
        ControlEventKind, EventSource, SaveTarget, SessionMetadata, DEFAULT_BLACK_BOX_DIRECTORY,
        DEFAULT_BLACK_BOX_MEMORY_BUDGET,
    };
    use crate::controller::{self, apply_audio_message, AudioMessage, EngineTelemetry, Garbage};
    use crate::resample::{
        capture_channel, parse_resample_command, CaptureClock, DestinationSpec, FinishedCapture,
        ResampleCommand, ResampleDestination, ResampleRequest, ResampleSource, ResampleSourceSpec,
        ResampleState, ResampleStop, Resampler,
    };
    use crate::scope::{
        draw_panel_sequence, header_line, open_panel_sequence, peak_dbfs, raw_mode_line,
        release_panel_sequence, render_scope, reserve_panel_sequence, scope_window, trigger_start,
    };
    use crate::tui::{
        render as tui_render, route_input, ui_input_from_crossterm, TuiLayout, TuiSnapshot,
        TuiTerminalGuard, UiAction, UiInput, UiKey,
    };
    use crate::workstation::{build_snapshot, handle_action, RuntimeStatus, UiEffect, UiState};
    #[cfg(feature = "midi")]
    use crate::{
        connect_midi_input, list_midi_input_names, select_midi_port_index, MidiPortSelector,
    };
    use crate::{
        list_output_device_names, map_performance_key, open_stereo_output_stream,
        parse_pattern_json, pc_speaker_backend, protect_master, shift_octave,
        spawn_realtime_recording, CompiledSamplePlayback, EngineCommand, LiveSequencer,
        MultiTrackEngine, MultiTrackProject, Oscillator, PanelLayout, Pattern, PatternStep,
        PeakHistory, PerformanceKey, PerformanceMix, PreparedTrack, QuantizeBoundary,
        RealtimeSynth, SampleAssetBank, SampleContext, SampleMode, SampleSettings, ScopeTap,
        SessionController, StartupOptions, TrackDefinition, TrackId, TrackKind, WavRecordingConfig,
        WaveformStyle, XyPoint, DEFAULT_SAMPLE_MEMORY_BUDGET,
    };
    use crossbeam_channel::{bounded, Receiver, Sender};
    use crossterm::event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
        KeyboardEnhancementFlags, MouseButton, MouseEventKind, PopKeyboardEnhancementFlags,
        PushKeyboardEnhancementFlags,
    };
    use crossterm::execute;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement};
    use std::collections::HashMap;
    use std::fs;
    use std::io::{self, Write};
    use std::path::{Path, PathBuf};
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    };
    use std::thread;
    use std::time::{Duration, Instant};

    const COMMAND_QUEUE_CAPACITY: usize = 256;
    const SEQUENCER_CONTROL_QUEUE_CAPACITY: usize = 32;
    const PERFORMANCE_QUEUE_CAPACITY: usize = 32;
    const GARBAGE_QUEUE_CAPACITY: usize = 16;
    const TELEMETRY_INTERVAL_FRAMES: u64 = 256;
    const TUI_REDRAW_INTERVAL: Duration = Duration::from_millis(40);
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
    const WAVEFORM_REDRAW_INTERVAL: Duration = Duration::from_millis(33);

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

    fn write_terminal(text: &str) -> Result<(), String> {
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(text.as_bytes())
            .and_then(|()| stdout.flush())
            .map_err(|error| format!("failed to write to terminal: {error}"))
    }

    /// Print a status line while the terminal is in raw mode, where a bare
    /// `\n` would leave the next line starting under the end of this one.
    fn session_line(text: &str) {
        let _ = write_terminal(&raw_mode_line(text));
    }

    fn session_error_line(text: &str) {
        let mut stderr = io::stderr().lock();
        let _ = stderr
            .write_all(raw_mode_line(text).as_bytes())
            .and_then(|()| stderr.flush());
    }

    /// Optional ASCII waveform pinned to the bottom of the terminal. Normal
    /// status output keeps scrolling in the rows above it.
    struct WaveformPanel {
        tap: Arc<ScopeTap>,
        style: WaveformStyle,
        sample_rate: u32,
        layout: Option<PanelLayout>,
        history: PeakHistory,
        cursor: u64,
        scratch: Vec<f32>,
        last_draw: Instant,
    }

    impl WaveformPanel {
        fn new(tap: Arc<ScopeTap>, style: WaveformStyle, sample_rate: u32) -> Self {
            Self {
                tap,
                style,
                sample_rate,
                layout: None,
                history: PeakHistory::new(1),
                cursor: 0,
                scratch: Vec::with_capacity(crate::SCOPE_CAPACITY),
                last_draw: Instant::now(),
            }
        }

        fn is_visible(&self) -> bool {
            self.layout.is_some()
        }

        fn layout_for_current_terminal() -> Result<PanelLayout, String> {
            #[cfg(windows)]
            if !crossterm::ansi_support::supports_ansi() {
                return Err("this console does not support ANSI escape sequences".into());
            }
            let (columns, rows) = crossterm::terminal::size()
                .map_err(|error| format!("failed to query terminal size: {error}"))?;
            PanelLayout::for_terminal(columns, rows).ok_or_else(|| {
                format!(
                    "terminal is {columns}x{rows}; the waveform needs at least {}x{}",
                    crate::scope::MIN_TERMINAL_COLUMNS,
                    crate::scope::MIN_TERMINAL_ROWS
                )
            })
        }

        fn show(&mut self) -> Result<(), String> {
            if self.is_visible() {
                return Ok(());
            }
            let layout = Self::layout_for_current_terminal()?;
            write_terminal(&open_panel_sequence(&layout))?;
            self.activate(layout)
        }

        fn activate(&mut self, layout: PanelLayout) -> Result<(), String> {
            self.layout = Some(layout);
            self.history.clear();
            self.history.resize(usize::from(layout.columns));
            self.cursor = self.tap.written();
            self.tap.set_enabled(true);
            self.draw()
        }

        fn hide(&mut self) -> Result<(), String> {
            let Some(layout) = self.layout.take() else {
                return Ok(());
            };
            self.tap.set_enabled(false);
            write_terminal(&release_panel_sequence(&layout))
        }

        fn toggle(&mut self) -> Result<(), String> {
            if self.is_visible() {
                self.hide()
            } else {
                self.show()
            }
        }

        fn cycle_style(&mut self) -> Result<(), String> {
            self.style = self.style.next();
            self.history.clear();
            if self.is_visible() {
                self.draw()?;
            }
            Ok(())
        }

        /// Rebuild the panel for a new terminal size. Old row positions are
        /// meaningless after a resize, so the screen is cleared and the scroll
        /// region re-established from scratch.
        fn resize(&mut self) -> Result<(), String> {
            if !self.is_visible() {
                return Ok(());
            }
            self.layout = None;
            write_terminal("\x1b[r\x1b[2J\x1b[H")?;
            match Self::layout_for_current_terminal() {
                Ok(layout) => {
                    write_terminal(&reserve_panel_sequence(&layout))?;
                    self.activate(layout)
                }
                Err(error) => {
                    self.tap.set_enabled(false);
                    Err(format!("waveform hidden: {error}"))
                }
            }
        }

        fn tick(&mut self) -> Result<(), String> {
            if self.is_visible() && self.last_draw.elapsed() >= WAVEFORM_REDRAW_INTERVAL {
                self.draw()?;
            }
            Ok(())
        }

        fn draw(&mut self) -> Result<(), String> {
            let Some(layout) = self.layout else {
                return Ok(());
            };
            self.last_draw = Instant::now();
            let width = usize::from(layout.columns);
            let height = layout.wave_rows();
            let (wave, dbfs) = match self.style {
                WaveformStyle::Scope => {
                    let window = scope_window(self.sample_rate);
                    self.tap.copy_latest(window * 2, &mut self.scratch);
                    self.cursor = self.tap.written();
                    let start = trigger_start(&self.scratch, window);
                    let end = (start + window).min(self.scratch.len());
                    let view = &self.scratch[start..end];
                    (render_scope(view, width, height), peak_dbfs(view))
                }
                WaveformStyle::History => {
                    self.tap.copy_since(&mut self.cursor, &mut self.scratch);
                    self.history.push_block(&self.scratch);
                    (
                        self.history.render(width, height),
                        peak_dbfs(&[self.history.latest()]),
                    )
                }
            };
            let mut lines = Vec::with_capacity(wave.len() + 1);
            lines.push(header_line(self.style, dbfs, width));
            lines.extend(wave);
            write_terminal(&draw_panel_sequence(&layout, &lines))
        }
    }

    impl Drop for WaveformPanel {
        fn drop(&mut self) {
            let _ = self.hide();
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

    type StatusMessages = Vec<(String, bool)>;

    /// Control-side services shared by the line and full-screen interfaces:
    /// the session controller plus the resampler and black box, which own
    /// files and threads the controller must not touch.
    struct Services {
        controller: Option<SessionController>,
        resampler: Option<Resampler>,
        resample_normalize: bool,
        resample_destination: DestinationSpec,
        black_box: Option<BlackBoxHandle>,
        garbage: Receiver<Garbage>,
        telemetry: Arc<EngineTelemetry>,
        output_frame: Arc<AtomicU64>,
        sample_rate: u32,
        polyphony: usize,
        started: Instant,
    }

    impl Services {
        fn now_ms(&self) -> u64 {
            self.started.elapsed().as_millis() as u64
        }

        fn transport_frame(&self) -> u64 {
            self.telemetry.position_frame()
        }

        fn record(&self, source: EventSource, kind: ControlEventKind) {
            if let Some(black_box) = self.black_box.as_ref() {
                black_box.record_event(self.output_frame.load(Ordering::Relaxed), source, kind);
            }
        }

        fn execute_line(&mut self, line: &str) -> Result<String, String> {
            let line = line.trim();
            let head = line.split_whitespace().next().unwrap_or_default();
            let kind = if matches!(head, "resample" | "blackbox") {
                ControlEventKind::CaptureCommand {
                    command: line.chars().take(200).collect(),
                }
            } else {
                ControlEventKind::EditorCommand {
                    command: line.chars().take(200).collect(),
                }
            };
            self.record(EventSource::Cli, kind);
            match head {
                "resample" => self.resample_command(line),
                "blackbox" => self.black_box_command(line),
                _ => {
                    let frame = self.transport_frame();
                    let now = self.now_ms();
                    let controller = self.controller.as_mut().ok_or(
                        "editing commands need a project (--project); live keys still work",
                    )?;
                    controller.tick(now);
                    controller.execute(line, frame)
                }
            }
        }

        fn clock(&self) -> Result<CaptureClock, String> {
            let controller = self
                .controller
                .as_ref()
                .ok_or("resampling needs a project (--project)")?;
            Ok(CaptureClock {
                sample_rate: self.sample_rate,
                bpm: controller.project().bpm,
                steps_per_beat: u32::from(controller.project().steps_per_beat),
                beats_per_bar: 4,
            })
        }

        fn resample_command(&mut self, line: &str) -> Result<String, String> {
            let command = parse_resample_command(line)?;
            let clock = self.clock()?;
            let frame = self.transport_frame();
            let selected = self
                .controller
                .as_ref()
                .map(SessionController::selected_track);
            let destination = match self.resample_destination {
                DestinationSpec::AssetOnly => ResampleDestination::AssetOnly,
                DestinationSpec::NewTrack => ResampleDestination::NewSampleTrack,
                DestinationSpec::Replace(track) => ResampleDestination::ReplaceTrackAsset {
                    track: track.or(selected).ok_or("no track selected")?,
                },
            };
            let normalize = self.resample_normalize;
            let resampler = self
                .resampler
                .as_mut()
                .ok_or("resampling needs a project (--project)")?;
            let arm = |resampler: &mut Resampler,
                       source: ResampleSource,
                       start: QuantizeBoundary,
                       stop: ResampleStop| {
                resampler
                    .arm(
                        ResampleRequest {
                            source,
                            start_boundary: start,
                            stop,
                            normalize,
                            destination,
                        },
                        frame,
                        clock,
                    )
                    .map(|(start, stop)| match stop {
                        Some(stop) => format!("resample armed: frames {start}..{stop}"),
                        None => {
                            format!("resample armed from frame {start}; `resample stop` to finish")
                        }
                    })
            };
            match command {
                ResampleCommand::Arm { source, stop } => {
                    let source = match source {
                        ResampleSourceSpec::Master => ResampleSource::Master,
                        ResampleSourceSpec::Track(track) => {
                            ResampleSource::Track(track.or(selected).ok_or("no track selected")?)
                        }
                    };
                    arm(
                        resampler,
                        source,
                        QuantizeBoundary::Bar { beats_per_bar: 4 },
                        stop,
                    )
                }
                ResampleCommand::Start => arm(
                    resampler,
                    ResampleSource::Master,
                    QuantizeBoundary::Immediate,
                    ResampleStop::Manual {
                        boundary: QuantizeBoundary::Immediate,
                    },
                ),
                ResampleCommand::Stop(boundary) => resampler
                    .stop(frame, clock, boundary)
                    .map(|frame| format!("resample stops at frame {frame}")),
                ResampleCommand::Cancel => resampler.cancel().map(|()| "resample cancelled".into()),
                ResampleCommand::Normalize(enabled) => {
                    self.resample_normalize = enabled;
                    Ok(format!(
                        "resample normalize {}",
                        if enabled { "on" } else { "off" }
                    ))
                }
                ResampleCommand::Destination(spec) => {
                    self.resample_destination = spec;
                    Ok(format!("resample destination {spec:?}"))
                }
                ResampleCommand::Status => Ok(describe_resample(resampler.state())),
            }
        }

        fn black_box_command(&mut self, line: &str) -> Result<String, String> {
            let parts: Vec<&str> = line.split_whitespace().collect();
            let black_box = self
                .black_box
                .as_ref()
                .ok_or("black box is off; start with --black-box-seconds <1-120> to enable it")?;
            match parts.get(1).copied() {
                None | Some("status") => Ok(describe_black_box(&black_box.status())),
                Some("save") => {
                    let target = match parts.get(2) {
                        Some(path) => SaveTarget::Path(PathBuf::from(path)),
                        None => SaveTarget::QuickSave,
                    };
                    let id = black_box
                        .request_save(target, self.output_frame.load(Ordering::Relaxed))?;
                    Ok(format!("black box save #{id} requested"))
                }
                Some("clear") => black_box
                    .clear()
                    .map(|()| "black box history cleared".into()),
                Some(other) => Err(format!("unknown blackbox command {other}")),
            }
        }

        fn black_box_save(&mut self) -> Result<String, String> {
            self.black_box_command("blackbox save")
        }

        /// Background bookkeeping, once per UI loop iteration.
        fn poll(&mut self) -> StatusMessages {
            let mut messages = Vec::new();
            while self.garbage.try_recv().is_ok() {}
            let now = self.now_ms();
            let frame = self.transport_frame();
            let snapshot_scene = self.telemetry.snapshot().active_scene;
            if let Some(controller) = self.controller.as_mut() {
                if let Some(message) = controller.tick(now) {
                    messages.push((message, false));
                }
                controller.observe_active_scene(snapshot_scene);
            }
            if let Some(finished) = self.resampler.as_mut().and_then(|r| r.poll(frame)) {
                match self.register_capture(finished) {
                    Ok(message) => messages.push((message, false)),
                    Err(error) => messages.push((error, true)),
                }
            }
            if let Some(ResampleState::Failed { message }) =
                self.resampler.as_ref().map(Resampler::state)
            {
                messages.push((format!("resample failed: {message}"), true));
                if let Some(resampler) = self.resampler.as_mut() {
                    resampler.acknowledge();
                }
            }
            if let Some(black_box) = self.black_box.as_ref() {
                while let Some(outcome) = black_box.try_outcome() {
                    messages.push(match outcome.result {
                        Ok(take) => (
                            format!(
                                "black box saved {} ({}){}",
                                take.wav_path.display(),
                                if take.complete {
                                    "complete"
                                } else {
                                    "INCOMPLETE: gaps recorded"
                                },
                                ""
                            ),
                            !take.complete,
                        ),
                        Err(error) => (format!("black box save failed: {error}"), true),
                    });
                }
            }
            messages
        }

        /// Make a finished capture playable according to its destination.
        /// Failures leave the WAV on disk as an asset-only result.
        fn register_capture(&mut self, finished: FinishedCapture) -> Result<String, String> {
            let mut notes = String::new();
            if finished.degraded {
                notes.push_str(" (DEGRADED: dropped blocks)");
            }
            if finished.silent {
                notes.push_str(" (silent)");
            }
            let path = finished.path.clone();
            let shown = path.display().to_string();
            let destination = finished.request.destination;
            let result = match destination {
                ResampleDestination::AssetOnly => Ok(format!("resample ready: {shown}{notes}")),
                ResampleDestination::NewSampleTrack => self.add_capture_track(finished),
                ResampleDestination::ReplaceTrackAsset { track } => {
                    self.replace_capture(track, finished)
                }
            };
            if let Some(resampler) = self.resampler.as_mut() {
                resampler.acknowledge();
            }
            result
                .map(|message| format!("{message}{notes}"))
                .map_err(|error| format!("resample kept as asset {shown}: {error}"))
        }

        fn add_capture_track(&mut self, finished: FinishedCapture) -> Result<String, String> {
            let controller = self.controller.as_mut().ok_or("no project")?;
            let id = controller.next_track_id();
            let project = controller.project();
            let (bpm, steps, seed) = (project.bpm, u32::from(project.steps_per_beat), project.seed);
            let mut steps_vec = vec![None; 16];
            steps_vec[0] = Some(PatternStep {
                note: 60,
                velocity: 1.0,
                gate: 1.0,
                probability: 1.0,
                ratchets: 1,
                microtiming_frames: 0,
            });
            let pattern = Pattern::new(format!("resample {}", id.0), seed, 0.0, 0, steps_vec)?;
            let mut definition =
                TrackDefinition::new(id, format!("Resample {}", id.0), TrackKind::Sample, pattern);
            let absolute = finished.path.clone();
            definition.sample = Some(SampleSettings::new(
                absolute.to_string_lossy(),
                SampleMode::OneShot,
            ));
            let mut bank = SampleAssetBank::new(usize::MAX);
            bank.insert_decoded(absolute.clone(), finished.decoded)?;
            let base = controller.project_dir();
            let prepared = PreparedTrack::new(
                self.sample_rate,
                bpm,
                steps,
                seed,
                self.polyphony,
                definition.clone(),
                Some(SampleContext {
                    assets: &bank,
                    base_dir: &base,
                }),
            )?;
            if let Some(sample) = definition.sample.as_mut() {
                sample.path = project_relative(&base, &absolute);
            }
            controller.add_sample_track(definition, prepared)
        }

        fn replace_capture(
            &mut self,
            track: TrackId,
            finished: FinishedCapture,
        ) -> Result<String, String> {
            let controller = self.controller.as_mut().ok_or("no project")?;
            let mut settings = controller
                .project()
                .tracks
                .iter()
                .find(|definition| definition.id == track)
                .and_then(|definition| definition.sample.clone())
                .ok_or_else(|| format!("track {} is not a sample track", track.0))?;
            let absolute = finished.path.clone();
            settings.path = absolute.to_string_lossy().into_owned();
            let mut bank = SampleAssetBank::new(usize::MAX);
            let asset = bank.insert_decoded(absolute.clone(), finished.decoded)?;
            let asset = bank.get(asset).ok_or("asset registration failed")?;
            let playback = CompiledSamplePlayback::new(asset, &settings, self.sample_rate)?;
            let base = controller.project_dir();
            controller.replace_track_sample(track, project_relative(&base, &absolute), playback)
        }

        fn status_text(&self) -> (Option<String>, Option<String>) {
            let resample = self
                .resampler
                .as_ref()
                .map(|resampler| describe_resample(resampler.state()))
                .filter(|text| text != "resample idle");
            let black_box = self.black_box.as_ref().map(|black_box| {
                let status = black_box.status();
                format!(
                    "BB {:.0}s{}",
                    status.effective_seconds,
                    if status.pending_saves > 0 {
                        " saving"
                    } else {
                        ""
                    }
                )
            });
            (resample, black_box)
        }

        fn shutdown(self) {
            if let Some(resampler) = self.resampler {
                resampler.shutdown();
            }
            if let Some(black_box) = self.black_box {
                let _ = black_box.shutdown(Duration::from_secs(5));
            }
            if let Some(controller) = self.controller {
                if controller.is_dirty() {
                    eprintln!(
                        "note: the project has unsaved changes (use `save` before quitting to keep them)"
                    );
                }
            }
        }
    }

    fn project_relative(base: &Path, path: &Path) -> String {
        path.strip_prefix(base)
            .ok()
            .filter(|relative| !base.as_os_str().is_empty() && !relative.as_os_str().is_empty())
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    fn describe_resample(state: &ResampleState) -> String {
        match state {
            ResampleState::Idle => "resample idle".into(),
            ResampleState::Armed { start_frame, .. } => {
                format!("resample armed (starts at frame {start_frame})")
            }
            ResampleState::Recording { .. } => "resample RECORDING".into(),
            ResampleState::Finalizing { .. } => "resample finalizing".into(),
            ResampleState::Ready { path } => format!("resample ready: {}", path.display()),
            ResampleState::Failed { message } => format!("resample failed: {message}"),
        }
    }

    fn describe_black_box(status: &BlackBoxStatus) -> String {
        format!(
            "black box: {} {:.1}s window, frames {}..{}, dropped audio frames {}, dropped events {}, pending saves {}{}",
            if status.armed { "armed" } else { "off" },
            status.effective_seconds,
            status.oldest_frame.map_or("-".into(), |frame| frame.to_string()),
            status.newest_frame.map_or("-".into(), |frame| frame.to_string()),
            status.dropped_audio_frames,
            status.dropped_events,
            status.pending_saves,
            status
                .failed
                .as_deref()
                .map_or(String::new(), |failure| format!(", FAILED: {failure}"))
        )
    }

    /// MIDI input with hot-plug reconnection, shared by both interfaces.
    #[cfg(feature = "midi")]
    struct MidiManager {
        selector: MidiPortSelector,
        handle: Option<crate::MidiInputHandle>,
        enabled: bool,
        last_scan: Instant,
    }

    #[cfg(feature = "midi")]
    impl MidiManager {
        fn new(options: &StartupOptions) -> Self {
            let selector = options.midi_port.clone().unwrap_or(MidiPortSelector::First);
            let handle = if options.no_midi {
                None
            } else {
                match connect_midi_input(&selector, MIDI_QUEUE_CAPACITY) {
                    Ok(handle) => Some(handle),
                    Err(error) => {
                        eprintln!(
                            "MIDI input unavailable: {error}; keyboard control remains active"
                        );
                        None
                    }
                }
            };
            Self {
                selector,
                handle,
                enabled: !options.no_midi,
                last_scan: Instant::now(),
            }
        }

        fn port_name(&self) -> Option<String> {
            self.handle
                .as_ref()
                .map(|handle| handle.port_name().to_string())
        }

        /// Drain MIDI, route it through mappings and the performance synth,
        /// and rescan ports once a second.
        fn poll(
            &mut self,
            services: &mut Services,
            command_sender: &Sender<EngineCommand>,
        ) -> Result<StatusMessages, String> {
            let mut messages = Vec::new();
            if let Some(handle) = self.handle.as_ref() {
                let port = handle.port_name().to_string();
                while let Some(event) = handle.try_recv() {
                    if let Some(kind) = midi_event_kind(event) {
                        services.record(EventSource::Midi, kind);
                    }
                    let consumed = match services.controller.as_mut() {
                        Some(controller) => {
                            let frame = services.telemetry.position_frame();
                            let disposition = controller.handle_midi(
                                &port,
                                event,
                                services.started.elapsed().as_millis() as u64,
                                frame,
                            );
                            messages.extend(disposition.messages.into_iter().map(|m| (m, false)));
                            disposition.consumed
                        }
                        None => false,
                    };
                    if !consumed {
                        if let Some(command) = engine_command_from_midi(event) {
                            send_command(command_sender, command)?;
                        }
                    }
                }
            }

            if self.enabled && self.last_scan.elapsed() >= Duration::from_secs(1) {
                self.last_scan = Instant::now();
                if let Ok(names) = list_midi_input_names() {
                    let desired = select_midi_port_index(&names, &self.selector)
                        .ok()
                        .and_then(|index| names.get(index).cloned());
                    let current = self.port_name();
                    if current != desired {
                        if current.is_some() {
                            self.handle = None;
                            let _ = send_command(command_sender, EngineCommand::Panic);
                            messages.push(("MIDI input disconnected".into(), true));
                        }
                        if desired.is_some() {
                            if let Ok(handle) =
                                connect_midi_input(&self.selector, MIDI_QUEUE_CAPACITY)
                            {
                                messages.push((
                                    format!("MIDI input connected: {}", handle.port_name()),
                                    false,
                                ));
                                self.handle = Some(handle);
                            }
                        }
                    }
                }
            }
            Ok(messages)
        }
    }

    #[cfg(feature = "midi")]
    fn midi_event_kind(event: crate::MidiEvent) -> Option<ControlEventKind> {
        match event {
            crate::MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            } => Some(ControlEventKind::NoteOn {
                channel,
                note,
                velocity: f32::from(velocity) / 127.0,
            }),
            crate::MidiEvent::NoteOff { channel, note, .. } => {
                Some(ControlEventKind::NoteOff { channel, note })
            }
            crate::MidiEvent::ControlChange {
                channel,
                controller,
                value,
            } => Some(ControlEventKind::ControlChange {
                channel,
                controller,
                value,
            }),
            crate::MidiEvent::PitchBend { .. } => None,
        }
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
        if options.tui && options.project_path.is_none() {
            return Err("the full-screen UI (--tui) needs a project; pass --project <FILE>".into());
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
        let project_dir = options
            .project_path
            .as_deref()
            .and_then(|path| Path::new(path).parent())
            .map(Path::to_path_buf)
            .unwrap_or_default();
        // Decode every sample before audio starts; a missing or invalid file
        // stops startup with a report of all of them.
        let sample_assets = project
            .as_ref()
            .map(|project| project.load_sample_assets(&project_dir, DEFAULT_SAMPLE_MEMORY_BUDGET))
            .transpose()
            .map_err(|error| format!("failed to load project samples: {error}"))?;
        let sample_memory = sample_assets
            .as_ref()
            .filter(|bank| !bank.is_empty())
            .map(|bank| (bank.len(), bank.used_bytes()));
        let has_sequencer = pattern.is_some() || project.is_some();

        let (command_sender, command_receiver) = bounded(COMMAND_QUEUE_CAPACITY);
        let (sequencer_sender, sequencer_receiver) = bounded(SEQUENCER_CONTROL_QUEUE_CAPACITY);
        let (performance_sender, performance_receiver) = bounded(PERFORMANCE_QUEUE_CAPACITY);
        let (audio_sender, audio_receiver) =
            bounded::<AudioMessage>(controller::AUDIO_MESSAGE_QUEUE_CAPACITY);
        let (garbage_sender, garbage_receiver) = bounded::<Garbage>(GARBAGE_QUEUE_CAPACITY);
        let (capture_sender, mut capture_tap, capture_returns) = capture_channel();
        let telemetry = Arc::new(EngineTelemetry::default());
        let audio_telemetry = Arc::clone(&telemetry);
        let scope_tap = Arc::new(ScopeTap::new());
        let audio_scope_tap = Arc::clone(&scope_tap);
        let output_frame = Arc::new(AtomicU64::new(0));
        let audio_output_frame = Arc::clone(&output_frame);
        let polyphony = options.polyphony;
        let bpm = options.bpm;
        let steps_per_beat = options.steps_per_beat;
        let record_path = options.record_path.clone();
        let black_box_seconds = options.black_box_seconds;
        let black_box_directory = PathBuf::from(
            options
                .black_box_directory
                .clone()
                .unwrap_or_else(|| DEFAULT_BLACK_BOX_DIRECTORY.into()),
        );
        let black_box_metadata = {
            let mut metadata = SessionMetadata::new(new_black_box_session_id());
            if let Some(project) = project.as_ref() {
                metadata.bpm = Some(project.bpm);
                metadata.steps_per_beat = Some(u32::from(project.steps_per_beat));
                metadata.seed = Some(project.seed);
            } else {
                metadata.bpm = Some(f64::from(bpm));
                metadata.steps_per_beat = Some(u32::from(steps_per_beat));
            }
            metadata
        };
        let engine_project = project.clone();
        let engine_project_dir = project_dir.clone();
        let (startup_sender, startup_receiver) = std::sync::mpsc::sync_channel(1);
        let audio =
            open_stereo_output_stream(options.audio_device.as_deref(), move |sample_rate| {
                let mut recorder = match record_path.as_deref() {
                    Some(path) => {
                        let config =
                            WavRecordingConfig::new(sample_rate, 1, RECORDING_QUEUE_CAPACITY)?;
                        let (producer, finalizer) =
                            spawn_realtime_recording(path, config, RECORDING_BLOCK_FRAMES)?;
                        Some((producer, finalizer))
                    }
                    None => None,
                };
                let (recording_producer, recording_finalizer) = match recorder.take() {
                    Some((producer, finalizer)) => (Some(producer), Some(finalizer)),
                    None => (None, None),
                };
                let mut recorder = recording_producer;
                // The memory budget is checked here, before audio starts.
                let black_box = match black_box_seconds {
                    Some(seconds) => {
                        let config = BlackBoxConfig::new(
                            seconds,
                            sample_rate,
                            black_box_directory.clone(),
                            DEFAULT_BLACK_BOX_MEMORY_BUDGET,
                        )?;
                        Some(spawn_black_box(config, black_box_metadata.clone())?)
                    }
                    None => None,
                };
                let (mut black_box_producer, black_box_handle) = match black_box {
                    Some((producer, handle)) => (Some(producer), Some(handle)),
                    None => (None, None),
                };
                startup_sender
                    .send((recording_finalizer, black_box_handle))
                    .map_err(|_| "failed to publish session state".to_string())?;

                let mut performance_synth =
                    RealtimeSynth::new(sample_rate as f32, Oscillator::Saw, polyphony)?;
                let mut multitrack = match engine_project.as_ref() {
                    Some(project) => Some(MultiTrackEngine::from_project(
                        sample_rate,
                        polyphony,
                        project,
                        sample_assets.as_ref().map(|assets| SampleContext {
                            assets,
                            base_dir: &engine_project_dir,
                        }),
                    )?),
                    None => None,
                };
                // Compiled playback holds its own references to the decoded
                // frames; the bank itself is not needed by the audio thread.
                drop(sample_assets);
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
                let mut frame_counter = 0_u64;

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

                    if let Some(engine) = multitrack.as_mut() {
                        for _ in 0..controller::AUDIO_MESSAGES_PER_FRAME {
                            let Ok(message) = audio_receiver.try_recv() else {
                                break;
                            };
                            apply_audio_message(engine, message, &garbage_sender);
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

                    capture_tap.poll_control();
                    let mut transport_frame = None;
                    let (sequencer_left, sequencer_right) =
                        if let Some(engine) = multitrack.as_mut() {
                            if engine.is_playing() {
                                transport_frame = Some(engine.position_frame());
                            }
                            engine.next_stereo_frame()
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
                    if let (Some(frame), Some(engine)) = (transport_frame, multitrack.as_ref()) {
                        // Resampling follows the transport: paused time is not captured.
                        let track = match capture_tap.source() {
                            Some(ResampleSource::Track(id)) => engine.track_output(id),
                            _ => None,
                        };
                        capture_tap.process_frame(frame, (left, right), track);
                    }
                    if let Some(recorder) = recorder.as_mut() {
                        let _ = recorder.push_sample(protect_master((left + right) * 0.5));
                    }
                    if let Some(producer) = black_box_producer.as_mut() {
                        producer.push_frame(frame_counter, left, right);
                    }
                    frame_counter = frame_counter.wrapping_add(1);
                    if frame_counter.is_multiple_of(TELEMETRY_INTERVAL_FRAMES) {
                        audio_output_frame.store(frame_counter, Ordering::Relaxed);
                        if let Some(engine) = multitrack.as_ref() {
                            audio_telemetry.publish(engine);
                        }
                    }
                    audio_scope_tap.push((left + right) * 0.5);
                    (left, right)
                })
            })?;
        let (recording_finalizer, black_box_handle) = startup_receiver
            .recv()
            .map_err(|_| "audio renderer did not publish session state".to_string())?;

        let controller = match project {
            Some(project) => Some(SessionController::new(
                project,
                options.project_path.as_ref().map(PathBuf::from),
                audio.sample_rate(),
                audio_sender,
            )?),
            None => None,
        };
        let capture_directory = options
            .capture_directory
            .as_ref()
            .map(PathBuf::from)
            .unwrap_or_else(|| project_dir.join("captures"));
        let resampler = controller.as_ref().map(|_| {
            Resampler::new(
                capture_directory,
                audio.sample_rate(),
                capture_sender,
                capture_returns,
            )
        });
        let mut services = Services {
            controller,
            resampler,
            resample_normalize: false,
            resample_destination: DestinationSpec::AssetOnly,
            black_box: black_box_handle,
            garbage: garbage_receiver,
            telemetry,
            output_frame,
            sample_rate: audio.sample_rate(),
            polyphony,
            started: Instant::now(),
        };

        #[cfg(feature = "midi")]
        let mut midi = MidiManager::new(&options);

        println!(
            "shelloop running: audio=\"{}\" {} Hz / {} ch, polyphony={} voices",
            audio.device_name(),
            audio.sample_rate(),
            audio.channels(),
            options.polyphony
        );
        #[cfg(feature = "midi")]
        if let Some(name) = midi.port_name() {
            println!("MIDI input: {name}");
        }
        if let Some(path) = options.record_path.as_deref() {
            println!("Recording mono master output to: {path}");
        }
        if let Some(black_box) = services.black_box.as_ref() {
            let config = black_box.config();
            println!(
                "Black box: keeping the last {:.1} s (requested {} s); F8 or `:blackbox save` writes to {}",
                config.effective_seconds(),
                config.requested_seconds,
                config.directory.display()
            );
        }
        if let Some(name) = pattern_name.as_deref() {
            println!(
                "Pattern: {name} at {} BPM, {} steps/beat",
                options.bpm, options.steps_per_beat
            );
            println!("Sequencer: Space play/pause, Backspace restart");
        }
        if let (Some(path), Some(controller)) = (
            options.project_path.as_deref(),
            services.controller.as_ref(),
        ) {
            let project = controller.project();
            println!(
                "Project: {path} ({} tracks, {} scenes, {} MIDI mappings)",
                project.tracks.len(),
                project.scenes.len(),
                project.midi_mappings.len()
            );
            if let Some((count, bytes)) = sample_memory {
                println!(
                    "Samples: {count} file(s) decoded, {:.1} MiB in memory",
                    bytes as f64 / (1024.0 * 1024.0)
                );
            }
            if !options.tui {
                println!("Sequencer: Space play/pause all tracks, Backspace restart all tracks");
                println!("Commands: press : then type `help` (tracks, steps, locks, scenes, fx, learn, resample, save)");
            }
        }
        if !options.tui {
            println!("Keys: Z-M/Q-U notes, [ ] octave, ! panic, ~ or Esc quit, F8 black-box save");
            println!("Waveform: Tab show/hide, Shift+Tab scope/history style");
        }
        if let Err(error) = io::stdout().flush() {
            request_panic(&command_sender, &sequencer_sender);
            drop(audio);
            if let Some(finalizer) = recording_finalizer {
                let _ = finalizer.finish();
            }
            return Err(format!("failed to flush terminal output: {error}"));
        }

        let channels = SessionChannels {
            command_sender: &command_sender,
            sequencer_sender: &sequencer_sender,
            performance_sender: &performance_sender,
            has_sequencer,
        };
        let session_result = if options.tui {
            run_tui_session(
                &options,
                &audio,
                &mut services,
                #[cfg(feature = "midi")]
                &mut midi,
                &channels,
                Arc::clone(&scope_tap),
            )
        } else {
            run_line_session(
                &options,
                &audio,
                &mut services,
                #[cfg(feature = "midi")]
                &mut midi,
                &channels,
                Arc::clone(&scope_tap),
            )
        };

        request_panic(&command_sender, &sequencer_sender);
        drop(audio);
        services.shutdown();

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

    struct SessionChannels<'a> {
        command_sender: &'a Sender<EngineCommand>,
        sequencer_sender: &'a Sender<SequencerControl>,
        performance_sender: &'a Sender<PerformanceMix>,
        has_sequencer: bool,
    }

    fn note_event(services: &Services, note: u8, on: bool) {
        services.record(
            EventSource::Keyboard,
            if on {
                ControlEventKind::NoteOn {
                    channel: KEYBOARD_CHANNEL,
                    note,
                    velocity: KEYBOARD_VELOCITY,
                }
            } else {
                ControlEventKind::NoteOff {
                    channel: KEYBOARD_CHANNEL,
                    note,
                }
            },
        );
    }

    fn run_line_session(
        options: &StartupOptions,
        audio: &crate::AudioOutput,
        services: &mut Services,
        #[cfg(feature = "midi")] midi: &mut MidiManager,
        channels: &SessionChannels<'_>,
        scope_tap: Arc<ScopeTap>,
    ) -> Result<(), String> {
        let command_sender = channels.command_sender;
        let sequencer_sender = channels.sequencer_sender;
        let has_sequencer = channels.has_sequencer;
        let terminal = TerminalGuard::enable(options.mouse_xy)?;
        if options.mouse_xy {
            session_error_line(
                "Mouse XY enabled: X crossfades live ↔ sequencer; Y controls overall level",
            );
        }
        if !terminal.release_events_supported() {
            session_error_line(
                "terminal does not expose key-release events; keyboard notes use a timed fallback. \
                 MIDI input or a terminal supporting the kitty keyboard protocol gives better note gating",
            );
        }

        let mut waveform =
            WaveformPanel::new(scope_tap, options.waveform_style, audio.sample_rate());
        if options.waveform {
            if let Err(error) = waveform.show() {
                session_error_line(&format!("waveform unavailable: {error}"));
            }
        }

        let commands_available = services.controller.is_some() || services.black_box.is_some();
        let mut octave = 0_i8;
        let mut held_notes = HashMap::<char, HeldNote>::new();
        let mut edit_command_buffer = None::<String>;

        let result = (|| -> Result<(), String> {
            'session: loop {
                if let Some(error) = audio.take_error() {
                    return Err(format!("audio stream error: {error}"));
                }
                waveform.tick()?;
                for (message, error) in services.poll() {
                    if error {
                        session_error_line(&message);
                    } else {
                        session_line(&message);
                    }
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
                            note_event(services, held.note, false);
                            send_command(
                                command_sender,
                                EngineCommand::NoteOff {
                                    channel: KEYBOARD_CHANNEL,
                                    note: held.note,
                                },
                            )?;
                        }
                    }
                }

                #[cfg(feature = "midi")]
                for (message, error) in midi.poll(services, command_sender)? {
                    if error {
                        session_error_line(&message);
                    } else {
                        session_line(&message);
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
                            services.record(
                                EventSource::Mouse,
                                ControlEventKind::XyMix {
                                    live_gain: mix.live_gain,
                                    sequencer_gain: mix.sequencer_gain,
                                },
                            );
                            let _ = channels.performance_sender.try_send(mix);
                        }
                        continue;
                    }
                    Event::Key(key_event) => key_event,
                    Event::Resize(..) => {
                        if let Err(error) = waveform.resize() {
                            session_error_line(&error);
                        }
                        continue;
                    }
                    _ => continue,
                };

                if let Some(mut buffer) = edit_command_buffer.take() {
                    if key_event.kind == KeyEventKind::Release {
                        edit_command_buffer = Some(buffer);
                        continue;
                    }

                    match key_event.code {
                        KeyCode::Esc => {
                            write_terminal("\r\n")?;
                        }
                        KeyCode::Enter => {
                            write_terminal("\r\n")?;
                            match services.execute_line(&buffer) {
                                Ok(message) if message.is_empty() => {}
                                Ok(message) => session_line(&message),
                                Err(error) => session_error_line(&format!("error: {error}")),
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
                    && commands_available
                {
                    edit_command_buffer = Some(String::new());
                    render_edit_prompt("")?;
                    continue;
                }

                if key_event.code == KeyCode::F(8) && key_event.kind == KeyEventKind::Press {
                    match services.black_box_save() {
                        Ok(message) => session_line(&message),
                        Err(error) => session_error_line(&error),
                    }
                    continue;
                }

                if key_event.code == KeyCode::Esc && key_event.kind != KeyEventKind::Release {
                    break 'session;
                }
                if key_event.kind == KeyEventKind::Press {
                    let shifted_tab = key_event.code == KeyCode::BackTab
                        || (key_event.code == KeyCode::Tab
                            && key_event.modifiers.contains(KeyModifiers::SHIFT));
                    let result = if shifted_tab {
                        Some(waveform.cycle_style())
                    } else if key_event.code == KeyCode::Tab {
                        Some(waveform.toggle())
                    } else {
                        None
                    };
                    if let Some(result) = result {
                        if let Err(error) = result {
                            session_error_line(&format!("waveform unavailable: {error}"));
                        }
                        continue;
                    }
                }
                if key_event.code == KeyCode::Backspace
                    && key_event.kind == KeyEventKind::Press
                    && has_sequencer
                {
                    services.record(
                        EventSource::Keyboard,
                        ControlEventKind::Transport {
                            action: "restart".into(),
                        },
                    );
                    send_sequencer_control(sequencer_sender, SequencerControl::Restart)?;
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
                            note_event(services, note, true);
                            send_command(
                                command_sender,
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
                            services.record(EventSource::Keyboard, ControlEventKind::Panic);
                            send_command(command_sender, EngineCommand::Panic)?;
                            send_sequencer_control(sequencer_sender, SequencerControl::Panic)?;
                            held_notes.clear();
                        }
                        Some(PerformanceKey::Quit) => break 'session,
                        Some(PerformanceKey::TogglePlay) if has_sequencer => {
                            services.record(
                                EventSource::Keyboard,
                                ControlEventKind::Transport {
                                    action: "toggle_play".into(),
                                },
                            );
                            send_sequencer_control(sequencer_sender, SequencerControl::TogglePlay)?;
                        }
                        Some(PerformanceKey::TogglePlay) | None => {}
                    },
                    KeyEventKind::Release => {
                        if let Some(held) = held_notes.remove(&held_key) {
                            note_event(services, held.note, false);
                            send_command(
                                command_sender,
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
        drop(waveform);
        drop(terminal);
        result
    }

    /// Full-screen workstation loop (spec 08).
    fn run_tui_session(
        options: &StartupOptions,
        audio: &crate::AudioOutput,
        services: &mut Services,
        #[cfg(feature = "midi")] midi: &mut MidiManager,
        channels: &SessionChannels<'_>,
        _scope_tap: Arc<ScopeTap>,
    ) -> Result<(), String> {
        let guard = TuiTerminalGuard::enter(true)?;
        let keyboard_enhancement = supports_keyboard_enhancement().unwrap_or(false);
        if keyboard_enhancement {
            let _ = execute!(
                io::stdout(),
                PushKeyboardEnhancementFlags(
                    KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                        | KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                )
            );
        }
        let releases = cfg!(windows) || keyboard_enhancement;
        let backend = ratatui::backend::CrosstermBackend::new(io::stdout());
        let mut terminal = ratatui::Terminal::new(backend)
            .map_err(|error| format!("failed to start the full-screen UI: {error}"))?;
        let mut ui = UiState::new(options.ascii);
        ui.push_status("press ? for help, : for commands, Ctrl+Q to quit", false);
        if !releases {
            ui.push_status(
                "terminal does not report key releases; keyboard notes use a timed fallback",
                true,
            );
        }
        let mut layout = TuiLayout::compute(0, 0);
        let mut last_snapshot: Option<TuiSnapshot> = None;
        let mut last_draw = Instant::now() - TUI_REDRAW_INTERVAL;
        let mut fallback_releases: Vec<(char, Instant)> = Vec::new();
        let runtime_status = |services: &Services, midi_name: Option<String>| {
            let (resample, black_box) = services.status_text();
            RuntimeStatus {
                audio_device: audio.device_name().to_string(),
                sample_rate: audio.sample_rate(),
                midi: midi_name,
                recording: options
                    .record_path
                    .clone()
                    .map(|path| format!("REC {path}")),
                black_box,
                resample,
            }
        };

        let result = (|| -> Result<(), String> {
            loop {
                if let Some(error) = audio.take_error() {
                    return Err(format!("audio stream error: {error}"));
                }
                for (message, error) in services.poll() {
                    ui.push_status(message, error);
                }
                #[cfg(feature = "midi")]
                for (message, error) in midi.poll(services, channels.command_sender)? {
                    ui.push_status(message, error);
                }

                if !releases {
                    let now = Instant::now();
                    let expired: Vec<char> = fallback_releases
                        .iter()
                        .filter(|(_, deadline)| now >= *deadline)
                        .map(|(key, _)| *key)
                        .collect();
                    fallback_releases.retain(|(_, deadline)| now < *deadline);
                    for key in expired {
                        let input = UiInput::Key {
                            key: UiKey::Char(key),
                            ctrl: false,
                            alt: false,
                            shift: false,
                            release: true,
                        };
                        for action in route_input(ui.mode, &input, None, None) {
                            if let Some(controller) = services.controller.as_mut() {
                                for effect in handle_action(action, &mut ui, controller, 0) {
                                    if let UiEffect::NoteOff(note) = effect {
                                        note_event(services, note, false);
                                        send_command(
                                            channels.command_sender,
                                            EngineCommand::NoteOff {
                                                channel: KEYBOARD_CHANNEL,
                                                note,
                                            },
                                        )?;
                                    }
                                }
                            }
                        }
                    }
                }

                if last_draw.elapsed() >= TUI_REDRAW_INTERVAL {
                    last_draw = Instant::now();
                    if let Some(controller) = services.controller.as_ref() {
                        #[cfg(feature = "midi")]
                        let midi_name = midi.port_name();
                        #[cfg(not(feature = "midi"))]
                        let midi_name = None;
                        let status = runtime_status(services, midi_name);
                        let telemetry = services.telemetry.snapshot();
                        let snapshot = build_snapshot(controller, &telemetry, &mut ui, &status);
                        terminal
                            .draw(|frame| {
                                let area = frame.area();
                                layout = TuiLayout::compute(area.width, area.height);
                                tui_render(frame, &snapshot, &layout);
                            })
                            .map_err(|error| format!("failed to draw the UI: {error}"))?;
                        last_snapshot = Some(snapshot);
                    }
                }

                if !event::poll(Duration::from_millis(10))
                    .map_err(|error| format!("terminal event polling failed: {error}"))?
                {
                    continue;
                }
                let terminal_event = event::read()
                    .map_err(|error| format!("failed to read terminal event: {error}"))?;
                let Some(input) = ui_input_from_crossterm(&terminal_event) else {
                    continue;
                };
                if let UiInput::Resize { .. } = input {
                    let _ = terminal.autoresize();
                    last_draw = Instant::now() - TUI_REDRAW_INTERVAL;
                }
                let actions = route_input(ui.mode, &input, Some(&layout), last_snapshot.as_ref());
                let frame = services.transport_frame();
                for action in actions {
                    let is_note = matches!(action, UiAction::NoteOn { .. });
                    if let (UiAction::NoteOn { key, .. }, false) = (&action, releases) {
                        fallback_releases.retain(|(held, _)| held != key);
                        fallback_releases.push((*key, Instant::now() + FALLBACK_INITIAL_HOLD));
                    }
                    let Some(controller) = services.controller.as_mut() else {
                        break;
                    };
                    let effects = handle_action(action, &mut ui, controller, frame);
                    if is_note && effects.is_empty() && !releases {
                        // Auto-repeat of a held key extends the fallback hold.
                        if let Some(entry) = fallback_releases.last_mut() {
                            entry.1 = Instant::now() + FALLBACK_REPEAT_GRACE;
                        }
                    }
                    for effect in effects {
                        match effect {
                            UiEffect::Quit => return Ok(()),
                            UiEffect::NoteOn(note) => {
                                note_event(services, note, true);
                                send_command(
                                    channels.command_sender,
                                    EngineCommand::NoteOn {
                                        channel: KEYBOARD_CHANNEL,
                                        note,
                                        velocity: KEYBOARD_VELOCITY,
                                    },
                                )?;
                            }
                            UiEffect::NoteOff(note) => {
                                note_event(services, note, false);
                                send_command(
                                    channels.command_sender,
                                    EngineCommand::NoteOff {
                                        channel: KEYBOARD_CHANNEL,
                                        note,
                                    },
                                )?;
                            }
                            UiEffect::Panic => {
                                services.record(EventSource::Keyboard, ControlEventKind::Panic);
                                send_command(channels.command_sender, EngineCommand::Panic)?;
                                send_sequencer_control(
                                    channels.sequencer_sender,
                                    SequencerControl::Panic,
                                )?;
                                fallback_releases.clear();
                            }
                            UiEffect::BlackBoxSave => match services.black_box_save() {
                                Ok(message) => ui.push_status(message, false),
                                Err(error) => ui.push_status(error, true),
                            },
                            UiEffect::Command(line) => {
                                if matches!(line.trim(), "quit" | "exit") {
                                    return Ok(());
                                }
                                match services.execute_line(&line) {
                                    Ok(message) if message.is_empty() => {}
                                    Ok(message) => ui.push_status(message, false),
                                    Err(error) => ui.push_status(error, true),
                                }
                            }
                            UiEffect::XyPad { x, y } => {
                                let point = XyPoint {
                                    x: x.clamp(0.0, 1.0),
                                    y: y.clamp(0.0, 1.0),
                                };
                                let mix = PerformanceMix::from_xy(point, channels.has_sequencer);
                                services.record(
                                    EventSource::Mouse,
                                    ControlEventKind::XyMix {
                                        live_gain: mix.live_gain,
                                        sequencer_gain: mix.sequencer_gain,
                                    },
                                );
                                let _ = channels.performance_sender.try_send(mix);
                            }
                        }
                    }
                }
            }
        })();
        if keyboard_enhancement {
            let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        }
        drop(terminal);
        drop(guard);
        result
    }
}

#[cfg(all(feature = "realtime-audio", feature = "terminal-ui"))]
pub use live::run_realtime_session;
