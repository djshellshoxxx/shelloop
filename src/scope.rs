//! Optional ASCII waveform display.
//!
//! The audio callback writes the protected master output into a fixed-size,
//! lock-free [`ScopeTap`]. The terminal thread copies recent samples out of the
//! tap and renders them as plain ASCII rows. Nothing here allocates, locks or
//! blocks on the audio side; all formatting happens on the control thread.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

/// Samples retained by the tap. A power of two so the write index can be masked.
pub const SCOPE_CAPACITY: usize = 16_384;
const SCOPE_MASK: u64 = (SCOPE_CAPACITY as u64) - 1;

/// Signals quieter than this are drawn at their true size instead of being
/// stretched to fill the panel, so silence and noise stay flat.
pub const DISPLAY_FLOOR: f32 = 0.25;

/// Smallest terminal that can hold the waveform panel plus a usable scroll area.
pub const MIN_TERMINAL_ROWS: u16 = 12;
pub const MIN_TERMINAL_COLUMNS: u16 = 20;
const MIN_PANEL_ROWS: u16 = 6;
const MAX_PANEL_ROWS: u16 = 16;

/// Characters used for the waveform body and the centre line.
pub const WAVE_CHAR: char = '#';
pub const AXIS_CHAR: char = '.';

/// Lock-free single-producer sample tap shared between the audio callback and
/// the terminal thread.
///
/// Only the audio callback may call [`ScopeTap::push`]. Readers may observe a
/// slot that is being overwritten when they fall a whole buffer behind; that is
/// harmless for a display and never unsafe because every slot is an atomic.
#[derive(Debug)]
pub struct ScopeTap {
    samples: Box<[AtomicU32]>,
    written: AtomicU64,
    enabled: AtomicBool,
}

impl Default for ScopeTap {
    fn default() -> Self {
        Self::new()
    }
}

impl ScopeTap {
    pub fn new() -> Self {
        let samples = (0..SCOPE_CAPACITY)
            .map(|_| AtomicU32::new(0.0_f32.to_bits()))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            samples,
            written: AtomicU64::new(0),
            enabled: AtomicBool::new(false),
        }
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Real-time safe: two relaxed atomic stores and one release store.
    /// Does nothing while the display is switched off.
    #[inline]
    pub fn push(&self, sample: f32) {
        if !self.enabled.load(Ordering::Relaxed) {
            return;
        }
        let sample = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        let index = self.written.load(Ordering::Relaxed);
        self.samples[(index & SCOPE_MASK) as usize].store(sample.to_bits(), Ordering::Relaxed);
        self.written.store(index.wrapping_add(1), Ordering::Release);
    }

    /// Total samples pushed since creation.
    pub fn written(&self) -> u64 {
        self.written.load(Ordering::Acquire)
    }

    /// Replace `out` with the most recent `count` samples, oldest first.
    /// Fewer are returned if fewer have been written; `count` is capped at
    /// [`SCOPE_CAPACITY`].
    pub fn copy_latest(&self, count: usize, out: &mut Vec<f32>) {
        let end = self.written();
        let count = (count.min(SCOPE_CAPACITY) as u64).min(end);
        self.copy_range(end - count, end, out);
    }

    /// Replace `out` with every sample written since `cursor` and advance the
    /// cursor. If the reader fell more than a whole buffer behind, only the
    /// newest [`SCOPE_CAPACITY`] samples are returned.
    pub fn copy_since(&self, cursor: &mut u64, out: &mut Vec<f32>) {
        let end = self.written();
        let start = (*cursor)
            .min(end)
            .max(end.saturating_sub(SCOPE_CAPACITY as u64));
        self.copy_range(start, end, out);
        *cursor = end;
    }

    fn copy_range(&self, start: u64, end: u64, out: &mut Vec<f32>) {
        out.clear();
        out.extend((start..end).map(|index| {
            f32::from_bits(self.samples[(index & SCOPE_MASK) as usize].load(Ordering::Relaxed))
        }));
    }
}

/// How the waveform panel draws the signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveformStyle {
    /// Triggered oscilloscope view of the most recent few milliseconds.
    Scope,
    /// Scrolling peak history, newest on the right, like a DAW clip overview.
    History,
}

impl WaveformStyle {
    pub fn next(self) -> Self {
        match self {
            Self::Scope => Self::History,
            Self::History => Self::Scope,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Scope => "scope",
            Self::History => "history",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "scope" => Some(Self::Scope),
            "history" => Some(Self::History),
            _ => None,
        }
    }
}

/// Where the panel lives on screen. Rows are one-based terminal rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PanelLayout {
    /// Last row of the scrolling text area above the panel.
    pub scroll_bottom: u16,
    /// First panel row (the header line).
    pub first_row: u16,
    /// Header plus waveform rows.
    pub rows: u16,
    pub columns: u16,
}

impl PanelLayout {
    /// Reserve roughly the bottom third of the terminal. Returns `None` when
    /// the terminal is too small to show both the panel and the normal output.
    pub fn for_terminal(columns: u16, rows: u16) -> Option<Self> {
        if columns < MIN_TERMINAL_COLUMNS || rows < MIN_TERMINAL_ROWS {
            return None;
        }
        let panel = (rows / 3).clamp(MIN_PANEL_ROWS, MAX_PANEL_ROWS);
        Some(Self {
            scroll_bottom: rows - panel,
            first_row: rows - panel + 1,
            rows: panel,
            columns,
        })
    }

    /// Rows available for the waveform itself, below the header.
    pub fn wave_rows(&self) -> usize {
        usize::from(self.rows.saturating_sub(1))
    }
}

/// Number of samples the oscilloscope view spans: about 40 ms, so a few cycles
/// of a bass note or a single cycle of a very low one remain readable.
pub fn scope_window(sample_rate: u32) -> usize {
    ((sample_rate as usize) / 25).clamp(64, SCOPE_CAPACITY / 2)
}

/// Start index of a `window`-sample view that begins on a rising zero
/// crossing, which keeps periodic waveforms still between redraws instead of
/// drifting. Prefers the crossing closest to the newest data. Falls back to the
/// newest `window` samples when there is no crossing.
pub fn trigger_start(samples: &[f32], window: usize) -> usize {
    if samples.len() <= window {
        return 0;
    }
    let latest = samples.len() - window;
    (1..=latest)
        .rev()
        .find(|&index| samples[index - 1] < 0.0 && samples[index] >= 0.0)
        .unwrap_or(latest)
}

fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .filter(|sample| sample.is_finite())
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()))
}

fn display_gain(peak: f32) -> f32 {
    1.0 / peak.max(DISPLAY_FLOOR)
}

fn value_row(value: f32, height: usize) -> usize {
    let value = value.clamp(-1.0, 1.0);
    let row = ((1.0 - value) * 0.5 * (height - 1) as f32).round() as usize;
    row.min(height - 1)
}

fn empty_grid(width: usize, height: usize) -> Vec<Vec<char>> {
    let axis = (height - 1) / 2;
    (0..height)
        .map(|row| {
            let fill = if row == axis { AXIS_CHAR } else { ' ' };
            vec![fill; width]
        })
        .collect()
}

fn grid_to_lines(grid: Vec<Vec<char>>) -> Vec<String> {
    grid.into_iter()
        .map(|row| row.into_iter().collect())
        .collect()
}

/// Draw samples as an oscilloscope trace, one column per slice of samples.
/// Each column fills every row between that slice's minimum and maximum so
/// fast waveforms read as solid shapes rather than scattered dots.
pub fn render_scope(samples: &[f32], width: usize, height: usize) -> Vec<String> {
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let mut grid = empty_grid(width, height);
    if samples.is_empty() {
        return grid_to_lines(grid);
    }

    let gain = display_gain(peak(samples));
    let len = samples.len();
    for column in 0..width {
        let start = column * len / width;
        let end = ((column + 1) * len / width).max(start + 1).min(len);
        let slice = &samples[start.min(len - 1)..end];
        let (low, high) = slice
            .iter()
            .map(|sample| if sample.is_finite() { *sample } else { 0.0 })
            .fold((f32::MAX, f32::MIN), |(low, high), sample| {
                (low.min(sample), high.max(sample))
            });
        let top = value_row(high * gain, height);
        let bottom = value_row(low * gain, height);
        for row in grid.iter_mut().take(bottom + 1).skip(top) {
            row[column] = WAVE_CHAR;
        }
    }
    grid_to_lines(grid)
}

/// Scrolling peak history: one column per redraw, newest on the right.
#[derive(Debug, Clone)]
pub struct PeakHistory {
    peaks: VecDeque<f32>,
    capacity: usize,
}

impl PeakHistory {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            peaks: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub fn push_block(&mut self, samples: &[f32]) {
        if self.peaks.len() == self.capacity {
            self.peaks.pop_front();
        }
        self.peaks.push_back(peak(samples).min(1.0));
    }

    /// Change the number of retained columns, keeping the newest.
    pub fn resize(&mut self, capacity: usize) {
        self.capacity = capacity.max(1);
        while self.peaks.len() > self.capacity {
            self.peaks.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.peaks.clear();
    }

    pub fn len(&self) -> usize {
        self.peaks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peaks.is_empty()
    }

    pub fn latest(&self) -> f32 {
        self.peaks.back().copied().unwrap_or(0.0)
    }

    pub fn render(&self, width: usize, height: usize) -> Vec<String> {
        if width == 0 || height == 0 {
            return Vec::new();
        }
        let mut grid = empty_grid(width, height);
        let gain = display_gain(self.peaks.iter().copied().fold(0.0, f32::max));
        let shown = self.peaks.len().min(width);
        let first_column = width - shown;
        for (offset, peak) in self.peaks.iter().skip(self.peaks.len() - shown).enumerate() {
            let level = peak * gain;
            if level <= 0.0 {
                continue;
            }
            let top = value_row(level, height);
            let bottom = value_row(-level, height);
            for row in grid.iter_mut().take(bottom + 1).skip(top) {
                row[first_column + offset] = WAVE_CHAR;
            }
        }
        grid_to_lines(grid)
    }
}

/// Peak level in dBFS for the header, floored at -96 dB so silence prints a
/// number instead of negative infinity.
pub fn peak_dbfs(samples: &[f32]) -> f32 {
    let peak = peak(samples);
    if peak <= 0.000_015_85 {
        -96.0
    } else {
        20.0 * peak.log10()
    }
}

/// One-line panel header, padded or truncated to exactly `width` characters.
pub fn header_line(style: WaveformStyle, dbfs: f32, width: usize) -> String {
    let text = format!(
        "-- wave: {:<7} peak {:>6.1} dBFS -- Tab hide, Shift+Tab style ",
        style.label(),
        dbfs.max(-96.0)
    );
    fit_width(&text, width, '-')
}

/// Pad with `fill` or truncate so the result is exactly `width` characters.
pub fn fit_width(text: &str, width: usize, fill: char) -> String {
    let mut line: String = text.chars().take(width).collect();
    let used = line.chars().count();
    line.extend(std::iter::repeat_n(fill, width - used));
    line
}

/// Escape sequence that confines normal scrolling output to the rows above
/// the panel, leaving the cursor where it was.
pub fn reserve_panel_sequence(layout: &PanelLayout) -> String {
    format!("\x1b7\x1b[1;{}r\x1b8", layout.scroll_bottom)
}

/// Escape sequence used when the panel first opens: scrolls existing output up
/// out of the panel rows, parks the cursor on the last scrolling row, then
/// reserves the panel. Without the scroll the cursor could be left inside the
/// panel, where later output would overwrite the waveform instead of scrolling.
pub fn open_panel_sequence(layout: &PanelLayout) -> String {
    let mut out = "\r\n".repeat(usize::from(layout.rows));
    out.push_str(&format!("\x1b[{}A", layout.rows));
    out.push_str(&reserve_panel_sequence(layout));
    out
}

/// Escape sequence that draws `lines` into the panel without moving the
/// visible cursor. Lines must already be at most `layout.columns` wide.
pub fn draw_panel_sequence(layout: &PanelLayout, lines: &[String]) -> String {
    let mut out = String::with_capacity(lines.len() * (usize::from(layout.columns) + 12) + 8);
    out.push_str("\x1b7");
    for (offset, line) in lines.iter().take(usize::from(layout.rows)).enumerate() {
        out.push_str(&format!(
            "\x1b[{};1H\x1b[2K{line}",
            layout.first_row + offset as u16
        ));
    }
    out.push_str("\x1b8");
    out
}

/// Escape sequence that releases the reserved rows back to normal scrolling
/// and blanks them.
pub fn release_panel_sequence(layout: &PanelLayout) -> String {
    let mut out = String::from("\x1b7\x1b[r");
    for row in layout.first_row..layout.first_row + layout.rows {
        out.push_str(&format!("\x1b[{row};1H\x1b[2K"));
    }
    out.push_str("\x1b8");
    out
}

/// A complete line for a terminal in raw mode, where `\n` alone does not
/// return the carriage. Clears whatever was on the line first (for example a
/// half-typed edit prompt).
pub fn raw_mode_line(text: &str) -> String {
    format!("\r\x1b[2K{text}\r\n")
}
