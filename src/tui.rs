//! Full-screen terminal UI building blocks (spec 08).
//!
//! The TUI is a pure client of the control model: a controller publishes a
//! [`TuiSnapshot`] roughly 20–30 times per second, [`render`] draws it, and
//! [`route_input`] turns terminal input into [`UiAction`]s which the controller
//! translates into typed engine/editor commands. Nothing in this module talks to
//! the audio thread.
//!
//! Everything except the ratatui renderer, the crossterm event conversion and
//! the real terminal guard is always compiled, so routing, layout, viewport,
//! glyph and restore logic are testable without a terminal.

use std::collections::VecDeque;
use std::ops::Range;

use crate::keyboard::{map_performance_key, PerformanceKey};
use crate::multitrack::{TrackId, TrackKind};
use crate::pattern::PatternStep;

// ===========================================================================
// Modes, input and actions
// ===========================================================================

/// Explicit UI focus. Key behaviour depends only on the mode, never on hidden
/// focus state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum UiMode {
    /// Transport, scene launching, mute/solo and computer-keyboard playing.
    #[default]
    Performance,
    /// Step navigation and editing on the selected track's pattern.
    PatternEdit,
    /// Parameter list for the selected track/step/effect.
    Inspector,
    /// `:` text command entry. Note keys type text here.
    Command,
    /// Waiting for a MIDI control to bind to the selected parameter.
    MidiLearn,
    /// Help overlay; any key closes it.
    Help,
}

impl UiMode {
    pub const ALL: [UiMode; 6] = [
        UiMode::Performance,
        UiMode::PatternEdit,
        UiMode::Inspector,
        UiMode::Command,
        UiMode::MidiLearn,
        UiMode::Help,
    ];

    /// Short upper-case label for the status line.
    pub fn label(self) -> &'static str {
        match self {
            UiMode::Performance => "PERFORM",
            UiMode::PatternEdit => "EDIT",
            UiMode::Inspector => "INSPECT",
            UiMode::Command => "COMMAND",
            UiMode::MidiLearn => "LEARN",
            UiMode::Help => "HELP",
        }
    }
}

/// Terminal-independent key code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiKey {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Tab,
    BackTab,
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Home,
    End,
    Delete,
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiMouseKind {
    /// Left button pressed.
    Down,
    /// Left button dragged.
    Drag,
    /// Left button released.
    Up,
    ScrollUp,
    ScrollDown,
}

/// Terminal-independent input event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiInput {
    Key {
        key: UiKey,
        ctrl: bool,
        alt: bool,
        shift: bool,
        /// True for key-release events (only reported by terminals with
        /// keyboard enhancement). Releases only matter for note keys.
        release: bool,
    },
    Mouse {
        column: u16,
        row: u16,
        kind: UiMouseKind,
    },
    Resize {
        width: u16,
        height: u16,
    },
}

impl UiInput {
    /// Plain key press without modifiers.
    pub fn key(key: UiKey) -> Self {
        UiInput::Key {
            key,
            ctrl: false,
            alt: false,
            shift: false,
            release: false,
        }
    }

    /// Plain character press without modifiers.
    pub fn char(c: char) -> Self {
        Self::key(UiKey::Char(c))
    }

    /// Character release without modifiers.
    pub fn char_release(c: char) -> Self {
        UiInput::Key {
            key: UiKey::Char(c),
            ctrl: false,
            alt: false,
            shift: false,
            release: true,
        }
    }
}

/// Which per-step value a [`UiAction::StepField`] edit changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StepField {
    Note,
    Velocity,
    Gate,
    Probability,
    Ratchets,
    Microtiming,
}

/// Intent produced by [`route_input`]. The controller maps these onto engine,
/// editor and runtime commands; the router itself does no audio work.
#[derive(Debug, Clone, PartialEq)]
pub enum UiAction {
    /// Switch to another mode.
    SetMode(UiMode),
    /// Leave the application (`~` in Performance mode, or Ctrl+Q / Ctrl+C).
    Quit,
    /// Escape at the top level: clear transient status, never quit.
    ClearStatus,
    /// Terminal size changed; the controller recomputes the layout.
    Resize {
        width: u16,
        height: u16,
    },

    // --- Performance ------------------------------------------------------
    /// Musical key pressed. `offset` is semitones above the current octave's
    /// C (0..=23); `key` is the lower-cased key so the matching release can be
    /// paired. Auto-repeat presses may arrive for a held key; the controller
    /// ignores NoteOn for a key that is already held.
    NoteOn {
        key: char,
        offset: u8,
    },
    /// Musical key released.
    NoteOff {
        key: char,
        offset: u8,
    },
    OctaveDown,
    OctaveUp,
    Panic,
    TogglePlay,
    /// Restart the transport from bar 1.
    Restart,
    /// Move the track selection by a relative amount.
    SelectTrack(i32),
    /// Select a track by its position in [`TuiSnapshot::tracks`].
    SelectTrackIndex(usize),
    /// Toggle mute of a specific track (stable id from the snapshot).
    ToggleMute(TrackId),
    /// Toggle solo of a specific track (stable id from the snapshot).
    ToggleSolo(TrackId),
    /// Toggle mute of the currently selected track.
    ToggleMuteSelected,
    /// Toggle solo of the currently selected track.
    ToggleSoloSelected,
    /// Launch the scene at this zero-based slot (F2 = slot 0 … F7 = slot 5).
    LaunchSceneSlot(usize),
    SceneNext,
    ScenePrev,
    /// Save the performance black-box buffer.
    BlackBoxSave,

    // --- Pattern edit -----------------------------------------------------
    CursorMove(i32),
    /// Absolute step index (from a grid click).
    CursorTo(usize),
    CursorHome,
    CursorEnd,
    /// Toggle the step under the cursor.
    ToggleStep,
    /// Nudge a field of the step under the cursor by `delta` units.
    StepField {
        field: StepField,
        delta: i32,
    },
    /// Scroll the grid viewport by this many steps.
    ScrollGrid(i32),

    // --- Inspector --------------------------------------------------------
    InspectorMove(i32),
    /// Adjust the selected parameter; `coarse` when Shift was held.
    InspectorAdjust {
        delta: i32,
        coarse: bool,
    },
    /// Arm MIDI learn for the selected inspector parameter.
    LearnSelectedParameter,
    /// Accept the captured MIDI control.
    ConfirmLearn,
    CancelLearn,
    /// Mouse position inside the XY pad, both normalised to 0..=1 (y=1 top).
    XyPad {
        x: f32,
        y: f32,
    },
    /// Mouse button released inside the XY pad.
    XyPadRelease,

    // --- Command ----------------------------------------------------------
    CommandInput(char),
    CommandBackspace,
    CommandSubmit,
    CommandCancel,
    /// -1 = older entry, +1 = newer entry.
    CommandHistory(i32),
    CommandComplete,
}

/// Key map shown in the help overlay. Keep in sync with [`route_input`].
pub const HELP_LINES: &[&str] = &[
    "GLOBAL   Ctrl+Q / Ctrl+C quit   Esc leave current mode (never quits)",
    "         ? or F1 help   : command line   F8 save black box",
    "         F2..F7 launch scene 1..6   F9 mute / F10 solo selected track",
    "PERFORM  z s x d c v g b h n j m  = C..B (lower octave)",
    "         q 2 w 3 e r 5 t 6 y 7 u  = C..B (upper octave)",
    "         [ ] octave   ! panic   Space play/pause   Backspace restart",
    "         < > previous/next scene   Up/Down track   ~ quit",
    "         Tab pattern edit   Shift+Tab inspector",
    "EDIT     Left/Right Home/End cursor   Enter/Space toggle step",
    "         n/N note  v/V velocity  g/G gate  p/P probability",
    "         r/R ratchets  t/T microtiming  (lower = down, upper = up)",
    "         PgUp/PgDn scroll 16 steps   Up/Down track   Tab inspector",
    "INSPECT  Up/Down select   Left/Right adjust (Shift = coarse)",
    "         l learn MIDI for selected parameter   Tab performance",
    "COMMAND  type text   Enter run   Up/Down history   Tab complete   Esc cancel",
    "LEARN    move a MIDI control, Enter confirm, Esc cancel",
    "MOUSE    click track name/M/S, grid steps (click again toggles), XY pad",
];

fn is_quit_chord(ctrl: bool, key: UiKey) -> bool {
    ctrl && matches!(key, UiKey::Char('q' | 'Q' | 'c' | 'C'))
}

/// Semitone offset of a musical key (0..=23), using the established
/// performance keyboard layout.
pub fn note_key_offset(c: char) -> Option<u8> {
    match map_performance_key(c, 0) {
        Some(PerformanceKey::Note(note)) => note.checked_sub(60),
        _ => None,
    }
}

/// Pure input router: maps one input event to zero or more actions.
///
/// `layout` and `snapshot` are only consulted for mouse events; without a
/// layout no mouse event produces an action.
pub fn route_input(
    mode: UiMode,
    input: &UiInput,
    layout: Option<&TuiLayout>,
    snapshot: Option<&TuiSnapshot>,
) -> Vec<UiAction> {
    match *input {
        UiInput::Resize { width, height } => vec![UiAction::Resize { width, height }],
        UiInput::Mouse { column, row, kind } => {
            route_mouse(mode, column, row, kind, layout, snapshot)
        }
        UiInput::Key {
            key,
            ctrl,
            alt,
            shift,
            release,
        } => {
            if release {
                return route_release(mode, key, ctrl, alt);
            }
            if is_quit_chord(ctrl, key) {
                return vec![UiAction::Quit];
            }
            match mode {
                UiMode::Performance => route_performance(key, ctrl, alt),
                UiMode::PatternEdit => route_pattern_edit(key, ctrl, alt),
                UiMode::Inspector => route_inspector(key, ctrl, alt, shift),
                UiMode::Command => route_command(key, ctrl, alt),
                UiMode::MidiLearn => match key {
                    UiKey::Esc => vec![
                        UiAction::CancelLearn,
                        UiAction::SetMode(UiMode::Performance),
                    ],
                    UiKey::Enter => vec![
                        UiAction::ConfirmLearn,
                        UiAction::SetMode(UiMode::Performance),
                    ],
                    _ => Vec::new(),
                },
                UiMode::Help => vec![UiAction::SetMode(UiMode::Performance)],
            }
        }
    }
}

fn route_release(mode: UiMode, key: UiKey, ctrl: bool, alt: bool) -> Vec<UiAction> {
    if mode != UiMode::Performance || ctrl || alt {
        return Vec::new();
    }
    match key {
        UiKey::Char(c) => note_key_offset(c)
            .map(|offset| {
                vec![UiAction::NoteOff {
                    key: c.to_ascii_lowercase(),
                    offset,
                }]
            })
            .unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Keys shared by the three non-text modes (Performance, PatternEdit,
/// Inspector).
fn route_shared(key: UiKey) -> Option<Vec<UiAction>> {
    let action = match key {
        UiKey::F(1) | UiKey::Char('?') => UiAction::SetMode(UiMode::Help),
        UiKey::Char(':') => UiAction::SetMode(UiMode::Command),
        UiKey::F(n @ 2..=7) => UiAction::LaunchSceneSlot(usize::from(n - 2)),
        UiKey::F(8) => UiAction::BlackBoxSave,
        UiKey::F(9) => UiAction::ToggleMuteSelected,
        UiKey::F(10) => UiAction::ToggleSoloSelected,
        _ => return None,
    };
    Some(vec![action])
}

fn route_performance(key: UiKey, ctrl: bool, alt: bool) -> Vec<UiAction> {
    if ctrl || alt {
        return Vec::new();
    }
    if let Some(actions) = route_shared(key) {
        return actions;
    }
    let action = match key {
        UiKey::Esc => UiAction::ClearStatus,
        UiKey::Tab => UiAction::SetMode(UiMode::PatternEdit),
        UiKey::BackTab => UiAction::SetMode(UiMode::Inspector),
        UiKey::Up => UiAction::SelectTrack(-1),
        UiKey::Down => UiAction::SelectTrack(1),
        UiKey::Backspace => UiAction::Restart,
        UiKey::Char('~') => UiAction::Quit,
        UiKey::Char('[') => UiAction::OctaveDown,
        UiKey::Char(']') => UiAction::OctaveUp,
        UiKey::Char('!') => UiAction::Panic,
        UiKey::Char(' ') => UiAction::TogglePlay,
        UiKey::Char('<') => UiAction::ScenePrev,
        UiKey::Char('>') => UiAction::SceneNext,
        UiKey::Char(c) => match note_key_offset(c) {
            Some(offset) => UiAction::NoteOn {
                key: c.to_ascii_lowercase(),
                offset,
            },
            None => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    vec![action]
}

fn route_pattern_edit(key: UiKey, ctrl: bool, alt: bool) -> Vec<UiAction> {
    if ctrl || alt {
        return Vec::new();
    }
    if let Some(actions) = route_shared(key) {
        return actions;
    }
    let field = |field: StepField, c: char| UiAction::StepField {
        field,
        delta: if c.is_ascii_uppercase() { 1 } else { -1 },
    };
    let action = match key {
        UiKey::Esc | UiKey::BackTab => UiAction::SetMode(UiMode::Performance),
        UiKey::Tab => UiAction::SetMode(UiMode::Inspector),
        UiKey::Left => UiAction::CursorMove(-1),
        UiKey::Right => UiAction::CursorMove(1),
        UiKey::Home => UiAction::CursorHome,
        UiKey::End => UiAction::CursorEnd,
        UiKey::Up => UiAction::SelectTrack(-1),
        UiKey::Down => UiAction::SelectTrack(1),
        UiKey::PageUp => UiAction::ScrollGrid(-16),
        UiKey::PageDown => UiAction::ScrollGrid(16),
        UiKey::Enter | UiKey::Char(' ') => UiAction::ToggleStep,
        UiKey::Char(c @ ('n' | 'N')) => field(StepField::Note, c),
        UiKey::Char(c @ ('v' | 'V')) => field(StepField::Velocity, c),
        UiKey::Char(c @ ('g' | 'G')) => field(StepField::Gate, c),
        UiKey::Char(c @ ('p' | 'P')) => field(StepField::Probability, c),
        UiKey::Char(c @ ('r' | 'R')) => field(StepField::Ratchets, c),
        UiKey::Char(c @ ('t' | 'T')) => field(StepField::Microtiming, c),
        _ => return Vec::new(),
    };
    vec![action]
}

fn route_inspector(key: UiKey, ctrl: bool, alt: bool, shift: bool) -> Vec<UiAction> {
    if ctrl || alt {
        return Vec::new();
    }
    if let Some(actions) = route_shared(key) {
        return actions;
    }
    let action = match key {
        UiKey::Esc | UiKey::Tab => UiAction::SetMode(UiMode::Performance),
        UiKey::BackTab => UiAction::SetMode(UiMode::PatternEdit),
        UiKey::Up => UiAction::InspectorMove(-1),
        UiKey::Down => UiAction::InspectorMove(1),
        UiKey::PageUp => UiAction::InspectorMove(-8),
        UiKey::PageDown => UiAction::InspectorMove(8),
        UiKey::Left => UiAction::InspectorAdjust {
            delta: -1,
            coarse: shift,
        },
        UiKey::Right => UiAction::InspectorAdjust {
            delta: 1,
            coarse: shift,
        },
        UiKey::Char('l') => {
            return vec![
                UiAction::LearnSelectedParameter,
                UiAction::SetMode(UiMode::MidiLearn),
            ]
        }
        _ => return Vec::new(),
    };
    vec![action]
}

fn route_command(key: UiKey, ctrl: bool, alt: bool) -> Vec<UiAction> {
    match key {
        UiKey::Esc => vec![
            UiAction::CommandCancel,
            UiAction::SetMode(UiMode::Performance),
        ],
        UiKey::Enter => vec![
            UiAction::CommandSubmit,
            UiAction::SetMode(UiMode::Performance),
        ],
        UiKey::Backspace => vec![UiAction::CommandBackspace],
        UiKey::Up => vec![UiAction::CommandHistory(-1)],
        UiKey::Down => vec![UiAction::CommandHistory(1)],
        UiKey::Tab => vec![UiAction::CommandComplete],
        UiKey::Char(c) if !ctrl && !alt && !c.is_control() => vec![UiAction::CommandInput(c)],
        _ => Vec::new(),
    }
}

fn route_mouse(
    mode: UiMode,
    column: u16,
    row: u16,
    kind: UiMouseKind,
    layout: Option<&TuiLayout>,
    snapshot: Option<&TuiSnapshot>,
) -> Vec<UiAction> {
    if !matches!(
        mode,
        UiMode::Performance | UiMode::PatternEdit | UiMode::Inspector
    ) {
        return Vec::new();
    }
    let Some(target) = layout.and_then(|layout| layout.hit_test(column, row, snapshot)) else {
        return Vec::new();
    };
    match (kind, target) {
        (
            UiMouseKind::Down,
            UiTarget::TrackRow {
                index,
                track,
                column_kind,
            },
        ) => {
            vec![match column_kind {
                TrackColumn::Name => UiAction::SelectTrackIndex(index),
                TrackColumn::Mute => UiAction::ToggleMute(track),
                TrackColumn::Solo => UiAction::ToggleSolo(track),
            }]
        }
        (UiMouseKind::Down, UiTarget::GridStep(step)) => {
            let cursor = snapshot.map(|snap| effective_viewport(&snap.grid).cursor);
            if cursor == Some(step) {
                vec![UiAction::CursorTo(step), UiAction::ToggleStep]
            } else {
                vec![UiAction::CursorTo(step)]
            }
        }
        (UiMouseKind::Down | UiMouseKind::Drag, UiTarget::XyPad { x, y }) => {
            vec![UiAction::XyPad { x, y }]
        }
        (UiMouseKind::Up, UiTarget::XyPad { .. }) => vec![UiAction::XyPadRelease],
        (UiMouseKind::ScrollUp, UiTarget::TrackRow { .. }) => vec![UiAction::SelectTrack(-1)],
        (UiMouseKind::ScrollDown, UiTarget::TrackRow { .. }) => vec![UiAction::SelectTrack(1)],
        (UiMouseKind::ScrollUp, UiTarget::GridStep(_)) => vec![UiAction::ScrollGrid(-4)],
        (UiMouseKind::ScrollDown, UiTarget::GridStep(_)) => vec![UiAction::ScrollGrid(4)],
        _ => Vec::new(),
    }
}

// ===========================================================================
// Grid viewport and playhead
// ===========================================================================

/// Default number of visible grid steps.
pub const GRID_VIEWPORT_STEPS: usize = 16;

/// Horizontal window over a pattern with a cursor that is always visible.
///
/// A `width` of 0 behaves like 1 so the cursor always has somewhere to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridViewport {
    pub len: usize,
    pub cursor: usize,
    pub offset: usize,
    pub width: usize,
}

impl Default for GridViewport {
    fn default() -> Self {
        Self::new(0, GRID_VIEWPORT_STEPS)
    }
}

impl GridViewport {
    pub fn new(len: usize, width: usize) -> Self {
        let mut viewport = Self {
            len,
            cursor: 0,
            offset: 0,
            width,
        };
        viewport.normalize();
        viewport
    }

    fn span(&self) -> usize {
        self.width.max(1)
    }

    fn max_offset(&self) -> usize {
        self.len.saturating_sub(self.span())
    }

    fn last_step(&self) -> usize {
        self.len.saturating_sub(1)
    }

    /// Clamp cursor/offset into range and scroll the cursor into view.
    fn normalize(&mut self) {
        self.cursor = self.cursor.min(self.last_step());
        if self.cursor < self.offset {
            self.offset = self.cursor;
        }
        let span = self.span();
        if self.cursor >= self.offset.saturating_add(span) {
            self.offset = self.cursor + 1 - span;
        }
        self.offset = self.offset.min(self.max_offset());
    }

    /// Change the pattern length, keeping the cursor valid and visible.
    pub fn set_len(&mut self, len: usize) {
        self.len = len;
        self.normalize();
    }

    pub fn move_cursor(&mut self, delta: i32) {
        self.cursor = offset_index(self.cursor, delta);
        self.normalize();
    }

    pub fn set_cursor(&mut self, index: usize) {
        self.cursor = index;
        self.normalize();
    }

    /// Scroll the window by `delta` steps; the cursor moves with the window.
    pub fn scroll(&mut self, delta: i32) {
        self.offset = offset_index(self.offset, delta).min(self.max_offset());
        let span = self.span();
        let last_visible = self.offset.saturating_add(span - 1).min(self.last_step());
        self.cursor =
            offset_index(self.cursor, delta).clamp(self.offset, last_visible.max(self.offset));
        self.normalize();
    }

    /// Absolute step indices currently visible.
    pub fn visible_range(&self) -> Range<usize> {
        let start = self.offset.min(self.len);
        start..self.offset.saturating_add(self.span()).min(self.len)
    }
}

fn offset_index(index: usize, delta: i32) -> usize {
    let magnitude = delta.unsigned_abs() as usize;
    if delta < 0 {
        index.saturating_sub(magnitude)
    } else {
        index.saturating_add(magnitude)
    }
}

/// Step under the playhead given the engine's frame position.
/// `None` for an empty pattern or a non-positive/non-finite step duration.
pub fn playhead_step(position_frame: u64, frames_per_step: f64, len: usize) -> Option<usize> {
    if len == 0 || !frames_per_step.is_finite() || frames_per_step <= 0.0 {
        return None;
    }
    let step = (position_frame as f64 / frames_per_step).floor();
    if !step.is_finite() || step < 0.0 {
        return None;
    }
    // Float-to-int casts saturate; the modulo keeps the result in range.
    Some(((step as u64) % len as u64) as usize)
}

// ===========================================================================
// Snapshot (pure data published by the controller)
// ===========================================================================

#[derive(Debug, Clone, PartialEq, Default)]
pub struct TransportView {
    pub playing: bool,
    pub bpm: f64,
    /// 1-based absolute bar.
    pub bar: u64,
    /// 1-based beat in bar.
    pub beat: u32,
    /// 1-based step within the beat.
    pub step_in_beat: u32,
    pub active_scene: Option<String>,
    /// Queued scene name and the bar it launches on.
    pub queued_scene: Option<(String, u64)>,
    pub chain: Option<String>,
    pub recording: Option<String>,
    pub black_box: Option<String>,
    pub midi: Option<String>,
    pub audio_device: String,
    pub sample_rate: u32,
    pub resample: Option<String>,
}

/// Active and possibly queued pattern revision of a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevisionState {
    Active(u64),
    Queued { active: u64, queued: u64 },
}

impl RevisionState {
    /// `rev 4` or `rev 4 (queued 5)`.
    pub fn label(&self) -> String {
        match *self {
            RevisionState::Active(active) => format!("rev {active}"),
            RevisionState::Queued { active, queued } => {
                format!("rev {active} (queued {queued})")
            }
        }
    }
}

impl Default for RevisionState {
    fn default() -> Self {
        RevisionState::Active(0)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TrackView {
    pub id: TrackId,
    pub name: String,
    pub kind: TrackKind,
    pub muted: bool,
    pub soloed: bool,
    /// False when muted or silenced by another track's solo.
    pub audible: bool,
    pub gain: f32,
    pub pan: f32,
    /// Latest peak, 0..1 (values above 1 indicate clipping).
    pub peak: f32,
    pub pattern_label: String,
    pub revision_state: RevisionState,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepView {
    pub note: u8,
    pub velocity: f32,
    pub gate: f32,
    pub probability: f32,
    pub ratchets: u8,
    pub microtiming: i32,
}

impl From<&PatternStep> for StepView {
    fn from(step: &PatternStep) -> Self {
        Self {
            note: step.note,
            velocity: step.velocity,
            gate: step.gate,
            probability: step.probability,
            ratchets: step.ratchets,
            microtiming: step.microtiming_frames,
        }
    }
}

/// The selected track's pattern.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GridView {
    pub steps: Vec<Option<StepView>>,
    /// Parameter-lock count per step (may be shorter than `steps`).
    pub lock_counts: Vec<u8>,
    pub viewport: GridViewport,
    pub playhead: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct InspectorView {
    pub title: String,
    pub rows: Vec<(String, String)>,
    pub selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusLine {
    pub text: String,
    pub error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommandView {
    pub buffer: String,
    pub completion: Option<String>,
}

/// Everything the renderer needs, built by the controller at UI cadence.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TuiSnapshot {
    pub transport: TransportView,
    pub tracks: Vec<TrackView>,
    pub selected_track: usize,
    pub grid: GridView,
    pub inspector: InspectorView,
    /// Oldest first; the renderer shows the newest lines that fit.
    pub status: Vec<StatusLine>,
    pub command: Option<CommandView>,
    pub mode: UiMode,
    /// Description of the parameter waiting for MIDI learn.
    pub learn: Option<String>,
    /// ASCII fallback instead of Unicode glyphs.
    pub ascii: bool,
}

/// The grid viewport made consistent with the actual step count.
fn effective_viewport(grid: &GridView) -> GridViewport {
    let mut viewport = grid.viewport;
    viewport.set_len(grid.steps.len());
    viewport
}

// ===========================================================================
// Glyphs and meters
// ===========================================================================

/// One-character state of a grid step. Meaning is carried by the glyph so a
/// monochrome terminal loses nothing; colour is decoration only.
///
/// | state                    | Unicode | ASCII |
/// |--------------------------|---------|-------|
/// | empty                    | `·`     | `.`   |
/// | empty under cursor       | `_`     | `_`   |
/// | active                   | `█`     | `X`   |
/// | probability < 1          | `▒`     | `?`   |
/// | ratchets > 1             | `≡`     | `R`   |
/// | lock-only (trigless)     | `◇`     | `L`   |
/// | playhead on empty step   | `│`     | `\|`  |
/// | playhead on any trigger  | `◆`     | `@`   |
///
/// Ratchets win over reduced probability. The cursor on a non-empty step is
/// shown by the caret line below the grid instead of changing the glyph.
pub fn step_glyph(
    step: Option<&StepView>,
    locks: u8,
    is_playhead: bool,
    is_cursor: bool,
    ascii: bool,
) -> char {
    let pick = |unicode: char, plain: char| if ascii { plain } else { unicode };
    match step {
        Some(_) if is_playhead => pick('◆', '@'),
        Some(step) if step.ratchets > 1 => pick('≡', 'R'),
        Some(step) if step.probability < 1.0 => pick('▒', '?'),
        Some(_) => pick('█', 'X'),
        None if is_playhead => pick('│', '|'),
        None if locks > 0 => pick('◇', 'L'),
        None if is_cursor => '_',
        None => pick('·', '.'),
    }
}

/// Fixed-width horizontal level meter. Above 1.0 the last cell becomes `!`.
pub fn meter_bar(peak: f32, width: usize, ascii: bool) -> String {
    if width == 0 {
        return String::new();
    }
    let level = if peak.is_finite() { peak.max(0.0) } else { 0.0 };
    let clipped = level > 1.0;
    let level = level.min(1.0);
    let mut out = String::with_capacity(width * 3);
    if ascii {
        let filled = ((level * width as f32).round() as usize).min(width);
        out.extend(std::iter::repeat_n('#', filled));
        out.extend(std::iter::repeat_n('.', width - filled));
    } else {
        const PARTIAL: [char; 8] = [' ', '▏', '▎', '▍', '▌', '▋', '▊', '▉'];
        let eighths = ((level * width as f32 * 8.0).round() as usize).min(width * 8);
        for cell in 0..width {
            let in_cell = eighths.saturating_sub(cell * 8).min(8);
            out.push(if in_cell == 8 {
                '█'
            } else {
                PARTIAL[in_cell]
            });
        }
    }
    if clipped {
        out.pop();
        out.push('!');
    }
    out
}

/// MIDI note name such as `C4` (middle C = 60).
pub fn note_name(note: u8) -> String {
    const NAMES: [&str; 12] = [
        "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
    ];
    let octave = i32::from(note / 12) - 1;
    format!("{}{octave}", NAMES[usize::from(note % 12)])
}

// ===========================================================================
// Command history and completion
// ===========================================================================

pub const COMMAND_HISTORY_CAPACITY: usize = 64;

/// Static first words offered by Tab completion.
pub const COMMAND_WORDS: &[&str] = &[
    "track",
    "step",
    "length",
    "swing",
    "rotate",
    "undo",
    "redo",
    "synth",
    "scene",
    "chain",
    "lock",
    "unlock",
    "locks",
    "learn",
    "unlearn",
    "mappings",
    "mapping",
    "fx",
    "send",
    "master",
    "resample",
    "blackbox",
    "variation",
    "save",
    "help",
    "quit",
];

/// Bounded command history with shell-like Up/Down navigation.
#[derive(Debug, Clone, Default)]
pub struct CommandHistory {
    entries: VecDeque<String>,
    /// Index into `entries` while browsing; `None` = editing a fresh line.
    position: Option<usize>,
}

impl CommandHistory {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a submitted command. Blank lines and immediate repeats are
    /// skipped; the oldest entry is dropped beyond the capacity. Resets
    /// browsing.
    pub fn push(&mut self, command: &str) {
        self.position = None;
        let command = command.trim();
        if command.is_empty() || self.entries.back().map(String::as_str) == Some(command) {
            return;
        }
        if self.entries.len() == COMMAND_HISTORY_CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(command.to_string());
    }

    /// Step to an older entry (stays on the oldest once reached).
    pub fn prev(&mut self) -> Option<&str> {
        if self.entries.is_empty() {
            return None;
        }
        let index = match self.position {
            None => self.entries.len() - 1,
            Some(index) => index.saturating_sub(1),
        };
        self.position = Some(index);
        self.entries.get(index).map(String::as_str)
    }

    /// Step to a newer entry; `None` once past the newest (fresh line).
    // Paired with `prev`; this is history navigation, not an iterator.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<&str> {
        let index = self.position? + 1;
        if index >= self.entries.len() {
            self.position = None;
            return None;
        }
        self.position = Some(index);
        self.entries.get(index).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Stop browsing (e.g. after the buffer was edited).
    pub fn reset(&mut self) {
        self.position = None;
    }
}

/// Complete `prefix` (a single word, no whitespace) against `candidates`.
///
/// Returns the longest common prefix of all matches when it is longer than
/// `prefix`, otherwise `None` (nothing to add, or ambiguous).
pub fn complete_command(prefix: &str, candidates: &[&str]) -> Option<String> {
    if prefix.is_empty() || prefix.chars().any(char::is_whitespace) {
        return None;
    }
    let mut matches = candidates
        .iter()
        .filter(|candidate| candidate.starts_with(prefix));
    let first = *matches.next()?;
    let mut common = first.len();
    for candidate in matches {
        common = first
            .char_indices()
            .zip(candidate.chars())
            .take_while(|((_, a), b)| a == b)
            .last()
            .map_or(0, |((index, c), _)| index + c.len_utf8())
            .min(common);
    }
    (common > prefix.len()).then(|| first[..common].to_string())
}

// ===========================================================================
// Layout and hit testing
// ===========================================================================

pub const MIN_TUI_COLUMNS: u16 = 60;
pub const MIN_TUI_ROWS: u16 = 18;

/// Column (inside the track pane border) of the mute glyph.
pub const TRACK_MUTE_COLUMN: u16 = 17;
/// Column (inside the track pane border) of the solo glyph.
pub const TRACK_SOLO_COLUMN: u16 = 19;
/// Column (inside the track pane border) where the meter starts.
pub const TRACK_METER_COLUMN: u16 = 21;
pub const TRACK_METER_WIDTH: usize = 4;
/// Row (inside the grid border) that holds the step glyphs.
pub const GRID_STEP_ROW: u16 = 1;

/// Terminal cell rectangle (independent of ratatui).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn contains(&self, column: u16, row: u16) -> bool {
        column >= self.x
            && row >= self.y
            && u32::from(column) < u32::from(self.x) + u32::from(self.width)
            && u32::from(row) < u32::from(self.y) + u32::from(self.height)
    }

    /// The area inside a one-cell border.
    pub fn inner(&self) -> Rect {
        if self.width < 2 || self.height < 2 {
            return Rect::new(self.x, self.y, 0, 0);
        }
        Rect::new(self.x + 1, self.y + 1, self.width - 2, self.height - 2)
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// Which part of a track row was hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackColumn {
    Name,
    Mute,
    Solo,
}

/// Interactive widget under the mouse.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiTarget {
    TrackRow {
        index: usize,
        track: TrackId,
        column_kind: TrackColumn,
    },
    /// Absolute step index.
    GridStep(usize),
    /// Normalised pad position, y = 1 at the top.
    XyPad { x: f32, y: f32 },
}

/// Screen regions. Bordered panes (`transport`, `tracks`, `grid`,
/// `inspector`) include their border; `xy_pad` is the interactive interior of
/// the pad (its frame is drawn one cell outside it); `status` is unbordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TuiLayout {
    pub transport: Rect,
    pub tracks: Rect,
    pub grid: Rect,
    pub inspector: Rect,
    pub xy_pad: Option<Rect>,
    pub status: Rect,
    /// Below the minimum size: only a transport line and a warning/status.
    pub compact: bool,
}

impl TuiLayout {
    pub fn compute(width: u16, height: u16) -> TuiLayout {
        if width < MIN_TUI_COLUMNS || height < MIN_TUI_ROWS {
            let transport_rows = height.min(1);
            return TuiLayout {
                transport: Rect::new(0, 0, width, transport_rows),
                status: Rect::new(0, transport_rows, width, height - transport_rows),
                compact: true,
                ..TuiLayout::default()
            };
        }
        let transport = Rect::new(0, 0, width, 3);
        let status = Rect::new(0, height - 2, width, 2);
        let body = height - 5;
        let inspector_rows = (body / 3).clamp(6, 10);
        let middle = body - inspector_rows;
        let tracks_width = ((u32::from(width) * 2 / 5).clamp(28, 52)) as u16;
        let tracks = Rect::new(0, 3, tracks_width, middle);
        let grid = Rect::new(tracks_width, 3, width - tracks_width, middle);
        let inspector = Rect::new(0, 3 + middle, width, inspector_rows);
        let inner = inspector.inner();
        let pad_frame_width = (inner.width / 3).min(26);
        let xy_pad = (pad_frame_width >= 8 && inner.height >= 3).then(|| {
            Rect::new(
                inner.x + inner.width - pad_frame_width,
                inner.y,
                pad_frame_width,
                inner.height,
            )
            .inner()
        });
        TuiLayout {
            transport,
            tracks,
            grid,
            inspector,
            xy_pad,
            status,
            compact: false,
        }
    }

    /// Map a cell to an interactive widget. Track rows and grid steps need a
    /// snapshot (for stable ids and the viewport); the XY pad does not.
    pub fn hit_test(
        &self,
        column: u16,
        row: u16,
        snapshot: Option<&TuiSnapshot>,
    ) -> Option<UiTarget> {
        if self.compact {
            return None;
        }
        if let Some(pad) = self.xy_pad.filter(|pad| pad.contains(column, row)) {
            let norm = |pos: u16, start: u16, len: u16| {
                if len <= 1 {
                    0.5
                } else {
                    (f32::from(pos - start) / f32::from(len - 1)).clamp(0.0, 1.0)
                }
            };
            return Some(UiTarget::XyPad {
                x: norm(column, pad.x, pad.width),
                y: 1.0 - norm(row, pad.y, pad.height),
            });
        }
        let snapshot = snapshot?;
        let tracks = self.tracks.inner();
        if tracks.contains(column, row) {
            let scroll = track_scroll(
                snapshot.selected_track,
                snapshot.tracks.len(),
                usize::from(tracks.height),
            );
            let index = scroll + usize::from(row - tracks.y);
            let track = snapshot.tracks.get(index)?;
            let column_kind = match column - tracks.x {
                TRACK_MUTE_COLUMN => TrackColumn::Mute,
                TRACK_SOLO_COLUMN => TrackColumn::Solo,
                _ => TrackColumn::Name,
            };
            return Some(UiTarget::TrackRow {
                index,
                track: track.id,
                column_kind,
            });
        }
        let grid = self.grid.inner();
        if grid.contains(column, row) && row - grid.y == GRID_STEP_ROW {
            let viewport = effective_viewport(&snapshot.grid);
            let visible = viewport.visible_range().len().min(usize::from(grid.width));
            let offset = usize::from(column - grid.x);
            if offset < visible {
                return Some(UiTarget::GridStep(viewport.offset + offset));
            }
        }
        None
    }
}

/// First visible track row so the selection stays on screen.
fn track_scroll(selected: usize, count: usize, visible: usize) -> usize {
    if visible == 0 || count == 0 {
        return 0;
    }
    let selected = selected.min(count - 1);
    (selected + 1)
        .saturating_sub(visible)
        .min(count.saturating_sub(visible))
}

fn track_kind_label(kind: TrackKind) -> &'static str {
    match kind {
        TrackKind::Synth => "syn",
        TrackKind::Sample => "smp",
        TrackKind::ExternalMidi => "ext",
    }
}

/// Text of one track-pane row; columns match [`TRACK_MUTE_COLUMN`] etc.
pub fn track_row_text(index: usize, track: &TrackView, selected: bool, ascii: bool) -> String {
    let number = if index < 99 {
        format!("{:02}", index + 1)
    } else {
        "++".to_string()
    };
    let name: String = track.name.chars().take(8).collect();
    format!(
        "{marker}{number} {name:<8} {kind} {mute} {solo} {meter} g{gain:.2} p{pan:+.2} {pattern}",
        marker = if selected { '>' } else { ' ' },
        kind = track_kind_label(track.kind),
        mute = if track.muted { 'M' } else { 'm' },
        solo = if track.soloed { 'S' } else { 's' },
        meter = meter_bar(track.peak, TRACK_METER_WIDTH, ascii),
        gain = finite_or_zero(track.gain),
        pan = finite_or_zero(track.pan),
        pattern = track.pattern_label,
    )
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        0.0
    }
}

/// One-line transport summary shared by the full and compact layouts.
pub fn transport_text(transport: &TransportView, ascii: bool) -> String {
    let mut parts = Vec::new();
    parts.push(match (transport.playing, ascii) {
        (true, false) => "▶ PLAY".to_string(),
        (true, true) => "> PLAY".to_string(),
        (false, false) => "■ PAUSE".to_string(),
        (false, true) => "= PAUSE".to_string(),
    });
    if transport.bpm.is_finite() {
        parts.push(format!("{:.1} BPM", transport.bpm));
    } else {
        parts.push("--- BPM".to_string());
    }
    parts.push(format!(
        "bar {}.{}.{}",
        transport.bar, transport.beat, transport.step_in_beat
    ));
    let arrow = if ascii { "->" } else { "→" };
    match (&transport.active_scene, &transport.queued_scene) {
        (Some(active), Some((queued, bar))) => {
            parts.push(format!("scene {active} {arrow} {queued}@{bar}"))
        }
        (Some(active), None) => parts.push(format!("scene {active}")),
        (None, Some((queued, bar))) => parts.push(format!("scene - {arrow} {queued}@{bar}")),
        (None, None) => {}
    }
    if let Some(chain) = &transport.chain {
        parts.push(format!("chain {chain}"));
    }
    if let Some(recording) = &transport.recording {
        parts.push(format!("REC {recording}"));
    }
    if let Some(resample) = &transport.resample {
        parts.push(format!("RESAMPLE {resample}"));
    }
    if let Some(black_box) = &transport.black_box {
        parts.push(format!("BB {black_box}"));
    }
    parts.push(format!(
        "MIDI {}",
        transport.midi.as_deref().unwrap_or("off")
    ));
    parts.push(format!(
        "audio {} {}Hz",
        transport.audio_device, transport.sample_rate
    ));
    parts.join("  ")
}

/// Header line of the pattern grid: label, viewport position and revision.
pub fn grid_header_text(snapshot: &TuiSnapshot) -> String {
    let viewport = effective_viewport(&snapshot.grid);
    let track = snapshot.tracks.get(snapshot.selected_track);
    let label = track.map_or("-", |track| track.pattern_label.as_str());
    let position = if viewport.len == 0 {
        "empty".to_string()
    } else {
        let range = viewport.visible_range();
        format!("{}-{}/{}", range.start + 1, range.end, viewport.len)
    };
    match track {
        Some(track) => format!("{label} {position} {}", track.revision_state.label()),
        None => format!("{label} {position}"),
    }
}

/// Detail line for the step under the cursor.
pub fn cursor_step_text(grid: &GridView) -> String {
    let viewport = effective_viewport(grid);
    if viewport.len == 0 {
        return "no steps".to_string();
    }
    let index = viewport.cursor;
    let locks = grid.lock_counts.get(index).copied().unwrap_or(0);
    match grid.steps.get(index).copied().flatten() {
        Some(step) => format!(
            "step {}: {} vel {:.2} gate {:.2} prob {:.2} rat {} mt {:+} locks {}",
            index + 1,
            note_name(step.note),
            step.velocity,
            step.gate,
            step.probability,
            step.ratchets,
            step.microtiming,
            locks
        ),
        None if locks > 0 => format!("step {}: rest, locks {locks}", index + 1),
        None => format!("step {}: rest", index + 1),
    }
}

// ===========================================================================
// Terminal restore logic (pure; the real guard is feature-gated)
// ===========================================================================

/// Primitive terminal operations, injectable for tests.
pub trait TerminalOps {
    fn enable_raw_mode(&mut self) -> Result<(), String>;
    fn disable_raw_mode(&mut self) -> Result<(), String>;
    fn enter_alternate_screen(&mut self) -> Result<(), String>;
    fn leave_alternate_screen(&mut self) -> Result<(), String>;
    fn hide_cursor(&mut self) -> Result<(), String>;
    fn show_cursor(&mut self) -> Result<(), String>;
    fn enable_mouse_capture(&mut self) -> Result<(), String>;
    fn disable_mouse_capture(&mut self) -> Result<(), String>;
}

/// Which terminal modes are currently applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TerminalState {
    pub raw: bool,
    pub alternate_screen: bool,
    pub cursor_hidden: bool,
    pub mouse_capture: bool,
}

/// Undo `state` in reverse order of setup. Every step is attempted even if an
/// earlier one fails; the first error is returned.
pub fn restore_terminal<O: TerminalOps + ?Sized>(
    ops: &mut O,
    state: TerminalState,
) -> Result<(), String> {
    let mut first_error = None;
    let mut record = |result: Result<(), String>| {
        if let Err(error) = result {
            first_error.get_or_insert(error);
        }
    };
    if state.mouse_capture {
        record(ops.disable_mouse_capture());
    }
    if state.cursor_hidden {
        record(ops.show_cursor());
    }
    if state.alternate_screen {
        record(ops.leave_alternate_screen());
    }
    if state.raw {
        record(ops.disable_raw_mode());
    }
    first_error.map_or(Ok(()), Err)
}

/// RAII terminal session over injectable [`TerminalOps`]: dropping it restores
/// every mode it applied.
pub struct TerminalSession<O: TerminalOps> {
    ops: Option<O>,
    state: TerminalState,
}

impl<O: TerminalOps> TerminalSession<O> {
    /// Raw mode, alternate screen, hidden cursor and (optionally) mouse
    /// capture. If any step fails, the steps already applied are undone and
    /// the error is returned together with the ops.
    pub fn enter(mut ops: O, mouse: bool) -> Result<Self, (String, O)> {
        let mut state = TerminalState::default();
        let result = (|| {
            ops.enable_raw_mode()?;
            state.raw = true;
            ops.enter_alternate_screen()?;
            state.alternate_screen = true;
            ops.hide_cursor()?;
            state.cursor_hidden = true;
            if mouse {
                ops.enable_mouse_capture()?;
                state.mouse_capture = true;
            }
            Ok::<(), String>(())
        })();
        match result {
            Ok(()) => Ok(Self {
                ops: Some(ops),
                state,
            }),
            Err(error) => {
                let _ = restore_terminal(&mut ops, state);
                Err((error, ops))
            }
        }
    }

    pub fn state(&self) -> TerminalState {
        self.state
    }

    /// Restore now (idempotent). Subsequent drops do nothing.
    pub fn restore(&mut self) -> Result<(), String> {
        let state = std::mem::take(&mut self.state);
        match self.ops.as_mut() {
            Some(ops) => restore_terminal(ops, state),
            None => Ok(()),
        }
    }

    /// Restore and hand back the ops (for tests and diagnostics).
    pub fn restore_and_take(mut self) -> Result<O, String> {
        self.restore()?;
        self.ops
            .take()
            .ok_or_else(|| "terminal ops already taken".to_string())
    }
}

impl<O: TerminalOps> Drop for TerminalSession<O> {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

#[cfg(feature = "terminal-ui")]
pub use term::{render, ui_input_from_crossterm, CrosstermTerminalOps, TuiTerminalGuard};

// ===========================================================================
// ratatui / crossterm integration
// ===========================================================================

#[cfg(feature = "terminal-ui")]
mod term {
    use super::*;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect as RRect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::widgets::{Block, Borders, Clear, Widget};
    use ratatui::Frame;

    fn to_ratatui(rect: Rect, area: RRect) -> RRect {
        RRect::new(rect.x, rect.y, rect.width, rect.height).intersection(area)
    }

    /// Write `text` on `row` of `rect` starting at `column`, clipped to the
    /// rect and the buffer.
    fn put(buf: &mut Buffer, rect: RRect, row: u16, column: u16, text: &str, style: Style) {
        let rect = rect.intersection(buf.area);
        if row >= rect.height || column >= rect.width {
            return;
        }
        let width = usize::from(rect.width - column);
        buf.set_stringn(rect.x + column, rect.y + row, text, width, style);
    }

    fn bordered(buf: &mut Buffer, rect: RRect, title: &str, focused: bool) -> RRect {
        let rect = rect.intersection(buf.area);
        if rect.width < 2 || rect.height < 2 {
            return RRect::new(rect.x, rect.y, 0, 0);
        }
        let style = if focused {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        let title = if focused {
            format!(" *{title} ")
        } else {
            format!(" {title} ")
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(style)
            .title(title);
        let inner = block.inner(rect);
        block.render(rect, buf);
        inner
    }

    /// Draw a snapshot. Never panics for any frame size or snapshot.
    pub fn render(frame: &mut Frame, snapshot: &TuiSnapshot, layout: &TuiLayout) {
        let area = frame.area();
        let buf = frame.buffer_mut();
        if layout.compact {
            render_compact(buf, area, snapshot, layout);
        } else {
            render_transport(buf, to_ratatui(layout.transport, area), snapshot);
            render_tracks(buf, to_ratatui(layout.tracks, area), snapshot);
            render_grid(buf, to_ratatui(layout.grid, area), snapshot);
            render_inspector(buf, area, snapshot, layout);
            render_status(buf, to_ratatui(layout.status, area), snapshot);
        }
        if snapshot.mode == UiMode::MidiLearn || snapshot.learn.is_some() {
            render_learn(buf, area, snapshot);
        }
        if snapshot.mode == UiMode::Help {
            render_help(buf, area);
        }
    }

    fn render_compact(buf: &mut Buffer, area: RRect, snapshot: &TuiSnapshot, layout: &TuiLayout) {
        let transport = to_ratatui(layout.transport, area);
        put(
            buf,
            transport,
            0,
            0,
            &transport_text(&snapshot.transport, snapshot.ascii),
            Style::default(),
        );
        let status = to_ratatui(layout.status, area);
        put(
            buf,
            status,
            0,
            0,
            &format!(
                "terminal too small: need {MIN_TUI_COLUMNS}x{MIN_TUI_ROWS} (have {}x{})",
                area.width, area.height
            ),
            Style::default().add_modifier(Modifier::BOLD),
        );
        let lines = status_lines(snapshot);
        let available = usize::from(status.height.saturating_sub(1));
        let skip = lines.len().saturating_sub(available);
        for (row, (text, style)) in lines.iter().skip(skip).enumerate() {
            put(buf, status, row as u16 + 1, 0, text, *style);
        }
    }

    fn render_transport(buf: &mut Buffer, rect: RRect, snapshot: &TuiSnapshot) {
        let inner = bordered(buf, rect, "Shelloop", false);
        let style = if snapshot.transport.playing {
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        put(
            buf,
            inner,
            0,
            0,
            &transport_text(&snapshot.transport, snapshot.ascii),
            style,
        );
    }

    fn render_tracks(buf: &mut Buffer, rect: RRect, snapshot: &TuiSnapshot) {
        let focused = snapshot.mode == UiMode::Performance;
        let inner = bordered(buf, rect, "Tracks", focused);
        if snapshot.tracks.is_empty() {
            put(buf, inner, 0, 0, "no tracks", Style::default());
            return;
        }
        let scroll = track_scroll(
            snapshot.selected_track,
            snapshot.tracks.len(),
            usize::from(inner.height),
        );
        for (row, (index, track)) in snapshot
            .tracks
            .iter()
            .enumerate()
            .skip(scroll)
            .take(usize::from(inner.height))
            .enumerate()
        {
            let selected = index == snapshot.selected_track;
            let mut style = Style::default();
            if selected {
                style = style.add_modifier(Modifier::REVERSED);
            }
            if !track.audible {
                style = style.add_modifier(Modifier::DIM);
            }
            let text = track_row_text(index, track, selected, snapshot.ascii);
            put(buf, inner, row as u16, 0, &text, style);
            if track.muted {
                put(
                    buf,
                    inner,
                    row as u16,
                    TRACK_MUTE_COLUMN,
                    "M",
                    style.fg(Color::Red).add_modifier(Modifier::BOLD),
                );
            }
            if track.soloed {
                put(
                    buf,
                    inner,
                    row as u16,
                    TRACK_SOLO_COLUMN,
                    "S",
                    style.fg(Color::Yellow).add_modifier(Modifier::BOLD),
                );
            }
        }
    }

    fn render_grid(buf: &mut Buffer, rect: RRect, snapshot: &TuiSnapshot) {
        let focused = snapshot.mode == UiMode::PatternEdit;
        let inner = bordered(buf, rect, "Pattern", focused);
        put(
            buf,
            inner,
            0,
            0,
            &grid_header_text(snapshot),
            Style::default(),
        );
        let grid = &snapshot.grid;
        let viewport = effective_viewport(grid);
        let range = viewport.visible_range();
        let visible = range.len().min(usize::from(inner.width));
        for (column, index) in range.take(visible).enumerate() {
            let step = grid.steps.get(index).copied().flatten();
            let locks = grid.lock_counts.get(index).copied().unwrap_or(0);
            let is_playhead = grid.playhead == Some(index);
            let is_cursor = viewport.cursor == index;
            let glyph = step_glyph(step.as_ref(), locks, is_playhead, is_cursor, snapshot.ascii);
            let mut style = Style::default();
            if is_playhead {
                style = style.fg(Color::Cyan).add_modifier(Modifier::BOLD);
            }
            if is_cursor && focused {
                style = style.add_modifier(Modifier::REVERSED);
            }
            let mut text = [0u8; 4];
            put(
                buf,
                inner,
                GRID_STEP_ROW,
                column as u16,
                glyph.encode_utf8(&mut text),
                style,
            );
        }
        if viewport.len > 0 {
            let caret_column = viewport.cursor - viewport.offset;
            if caret_column < visible {
                put(
                    buf,
                    inner,
                    GRID_STEP_ROW + 1,
                    caret_column as u16,
                    "^",
                    Style::default(),
                );
            }
        }
        put(
            buf,
            inner,
            GRID_STEP_ROW + 2,
            0,
            &cursor_step_text(grid),
            Style::default(),
        );
    }

    fn render_inspector(buf: &mut Buffer, area: RRect, snapshot: &TuiSnapshot, layout: &TuiLayout) {
        let focused = snapshot.mode == UiMode::Inspector;
        let title = if snapshot.inspector.title.is_empty() {
            "Inspector"
        } else {
            snapshot.inspector.title.as_str()
        };
        let inner = bordered(buf, to_ratatui(layout.inspector, area), title, focused);
        let pad_frame = layout.xy_pad.map(|pad| {
            to_ratatui(
                Rect::new(
                    pad.x.saturating_sub(1),
                    pad.y.saturating_sub(1),
                    pad.width.saturating_add(2),
                    pad.height.saturating_add(2),
                ),
                area,
            )
        });
        let rows_width = match pad_frame {
            Some(frame) => frame.x.saturating_sub(inner.x).saturating_sub(1),
            None => inner.width,
        };
        let rows_area = RRect::new(inner.x, inner.y, rows_width.min(inner.width), inner.height);
        let rows = &snapshot.inspector.rows;
        let height = usize::from(rows_area.height);
        if rows.is_empty() {
            put(buf, rows_area, 0, 0, "nothing to inspect", Style::default());
        } else if height > 0 {
            let selected = snapshot.inspector.selected.min(rows.len() - 1);
            let scroll = (selected + 1).saturating_sub(height);
            let name_width = rows
                .iter()
                .map(|(name, _)| name.chars().count())
                .max()
                .unwrap_or(0);
            for (row, (index, (name, value))) in rows
                .iter()
                .enumerate()
                .skip(scroll)
                .take(height)
                .enumerate()
            {
                let marker = if index == selected { '>' } else { ' ' };
                let style = if index == selected && focused {
                    Style::default().add_modifier(Modifier::REVERSED)
                } else {
                    Style::default()
                };
                let text = format!("{marker}{name:<name_width$}  {value}");
                put(buf, rows_area, row as u16, 0, &text, style);
            }
        }
        if let Some(frame) = pad_frame {
            let pad_inner = bordered(buf, frame, "XY", false);
            if pad_inner.width > 0 && pad_inner.height > 0 {
                let center = if snapshot.ascii { "+" } else { "┼" };
                put(
                    buf,
                    pad_inner,
                    pad_inner.height / 2,
                    pad_inner.width / 2,
                    center,
                    Style::default().add_modifier(Modifier::DIM),
                );
                put(
                    buf,
                    pad_inner,
                    0,
                    0,
                    "drag",
                    Style::default().add_modifier(Modifier::DIM),
                );
            }
        }
    }

    fn status_lines(snapshot: &TuiSnapshot) -> Vec<(String, Style)> {
        snapshot
            .status
            .iter()
            .map(|line| {
                if line.error {
                    (
                        format!("error: {}", line.text),
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    )
                } else {
                    (line.text.clone(), Style::default())
                }
            })
            .collect()
    }

    fn render_status(buf: &mut Buffer, rect: RRect, snapshot: &TuiSnapshot) {
        if rect.height == 0 {
            return;
        }
        let lines = status_lines(snapshot);
        let status_rows = rect.height.saturating_sub(1);
        let skip = lines.len().saturating_sub(usize::from(status_rows));
        for (row, (text, style)) in lines.iter().skip(skip).enumerate() {
            put(buf, rect, row as u16, 0, text, *style);
        }
        let bottom = rect.height - 1;
        let line = match (&snapshot.command, snapshot.mode) {
            (Some(command), _) => {
                let mut text = format!(":{}_", command.buffer);
                if let Some(completion) = &command.completion {
                    text.push_str(&format!("   [Tab] {completion}"));
                }
                text
            }
            (None, UiMode::Command) => ":_".to_string(),
            (None, mode) => format!(
                "[{}]  ? help  : command  Tab next pane  Esc back  ~ quit",
                mode.label()
            ),
        };
        put(buf, rect, bottom, 0, &line, Style::default());
    }

    fn centered(area: RRect, width: u16, height: u16) -> RRect {
        let width = width.min(area.width);
        let height = height.min(area.height);
        RRect::new(
            area.x + (area.width - width) / 2,
            area.y + (area.height - height) / 2,
            width,
            height,
        )
    }

    fn render_learn(buf: &mut Buffer, area: RRect, snapshot: &TuiSnapshot) {
        let target = snapshot.learn.as_deref().unwrap_or("selected parameter");
        let lines = [
            format!("Waiting for MIDI: {target}"),
            "Move a control, Enter confirm, Esc cancel".to_string(),
        ];
        let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16 + 4;
        let rect = centered(area, width, 4);
        Clear.render(rect, buf);
        let inner = bordered(buf, rect, "MIDI LEARN", true);
        for (row, line) in lines.iter().enumerate() {
            put(buf, inner, row as u16, 1, line, Style::default());
        }
        if inner.is_empty() {
            put(buf, area, 0, 0, "MIDI LEARN", Style::default());
        }
    }

    fn render_help(buf: &mut Buffer, area: RRect) {
        let width = HELP_LINES
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or(0) as u16
            + 4;
        let rect = centered(area, width, HELP_LINES.len() as u16 + 3);
        Clear.render(rect, buf);
        let inner = bordered(buf, rect, "Help", true);
        for (row, line) in HELP_LINES.iter().enumerate() {
            put(buf, inner, row as u16, 1, line, Style::default());
        }
        put(
            buf,
            inner,
            HELP_LINES.len() as u16,
            1,
            "press any key to close",
            Style::default().add_modifier(Modifier::DIM),
        );
    }

    /// Convert a crossterm event; unsupported events map to `None`.
    pub fn ui_input_from_crossterm(event: &crossterm::event::Event) -> Option<UiInput> {
        use crossterm::event::{
            Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind,
        };
        match event {
            Event::Key(key) => {
                let code = match key.code {
                    KeyCode::Char(c) => UiKey::Char(c),
                    KeyCode::Enter => UiKey::Enter,
                    KeyCode::Esc => UiKey::Esc,
                    KeyCode::Backspace => UiKey::Backspace,
                    KeyCode::Tab => UiKey::Tab,
                    KeyCode::BackTab => UiKey::BackTab,
                    KeyCode::Up => UiKey::Up,
                    KeyCode::Down => UiKey::Down,
                    KeyCode::Left => UiKey::Left,
                    KeyCode::Right => UiKey::Right,
                    KeyCode::PageUp => UiKey::PageUp,
                    KeyCode::PageDown => UiKey::PageDown,
                    KeyCode::Home => UiKey::Home,
                    KeyCode::End => UiKey::End,
                    KeyCode::Delete => UiKey::Delete,
                    KeyCode::F(n) => UiKey::F(n),
                    _ => return None,
                };
                Some(UiInput::Key {
                    key: code,
                    ctrl: key.modifiers.contains(KeyModifiers::CONTROL),
                    alt: key.modifiers.contains(KeyModifiers::ALT),
                    shift: key.modifiers.contains(KeyModifiers::SHIFT),
                    release: key.kind == KeyEventKind::Release,
                })
            }
            Event::Mouse(mouse) => {
                let kind = match mouse.kind {
                    MouseEventKind::Down(MouseButton::Left) => UiMouseKind::Down,
                    MouseEventKind::Drag(MouseButton::Left) => UiMouseKind::Drag,
                    MouseEventKind::Up(MouseButton::Left) => UiMouseKind::Up,
                    MouseEventKind::ScrollUp => UiMouseKind::ScrollUp,
                    MouseEventKind::ScrollDown => UiMouseKind::ScrollDown,
                    _ => return None,
                };
                Some(UiInput::Mouse {
                    column: mouse.column,
                    row: mouse.row,
                    kind,
                })
            }
            Event::Resize(width, height) => Some(UiInput::Resize {
                width: *width,
                height: *height,
            }),
            _ => None,
        }
    }

    /// [`TerminalOps`] on the process's stdout via crossterm.
    #[derive(Debug, Default)]
    pub struct CrosstermTerminalOps;

    fn exec(command: impl crossterm::Command) -> Result<(), String> {
        let mut out = std::io::stdout();
        crossterm::execute!(out, command).map_err(|error| error.to_string())
    }

    impl TerminalOps for CrosstermTerminalOps {
        fn enable_raw_mode(&mut self) -> Result<(), String> {
            crossterm::terminal::enable_raw_mode().map_err(|error| error.to_string())
        }
        fn disable_raw_mode(&mut self) -> Result<(), String> {
            crossterm::terminal::disable_raw_mode().map_err(|error| error.to_string())
        }
        fn enter_alternate_screen(&mut self) -> Result<(), String> {
            exec(crossterm::terminal::EnterAlternateScreen)
        }
        fn leave_alternate_screen(&mut self) -> Result<(), String> {
            exec(crossterm::terminal::LeaveAlternateScreen)
        }
        fn hide_cursor(&mut self) -> Result<(), String> {
            exec(crossterm::cursor::Hide)
        }
        fn show_cursor(&mut self) -> Result<(), String> {
            exec(crossterm::cursor::Show)
        }
        fn enable_mouse_capture(&mut self) -> Result<(), String> {
            exec(crossterm::event::EnableMouseCapture)
        }
        fn disable_mouse_capture(&mut self) -> Result<(), String> {
            exec(crossterm::event::DisableMouseCapture)
        }
    }

    /// RAII guard for the real terminal: alternate screen, raw mode, hidden
    /// cursor and optional mouse capture, all restored on drop (including
    /// early returns and unwinding panics).
    pub struct TuiTerminalGuard {
        session: TerminalSession<CrosstermTerminalOps>,
    }

    impl TuiTerminalGuard {
        pub fn enter(mouse: bool) -> Result<Self, String> {
            TerminalSession::enter(CrosstermTerminalOps, mouse)
                .map(|session| Self { session })
                .map_err(|(error, _)| format!("could not prepare the terminal: {error}"))
        }

        pub fn state(&self) -> TerminalState {
            self.session.state()
        }

        /// Restore explicitly (e.g. to report restore errors); drop is then a
        /// no-op.
        pub fn restore(&mut self) -> Result<(), String> {
            self.session.restore()
        }
    }
}
