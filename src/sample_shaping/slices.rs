//! Slice definitions. Regions are stored normalised (0.0..=1.0) in project
//! files so they survive a replaced sample; they are converted to frame
//! indices when a playback is compiled.

use crate::sample::StereoFrame;
use serde::{Deserialize, Serialize};

pub const MAX_SLICES: usize = 128;
pub const SLICES_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceDef {
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SliceSelect {
    /// Slices are stored but notes play the whole region (step locks can
    /// still pick a slice).
    #[default]
    None,
    /// Note `base_note + n` plays slice `n`; other notes are silent.
    Note,
    /// Each note-on picks a slice from a seeded, deterministic sequence.
    Random,
}

fn default_version() -> u32 {
    SLICES_VERSION
}
fn default_base_note() -> u8 {
    36
}
fn default_seed() -> u32 {
    1
}
fn is_default<T: Default + PartialEq>(value: &T) -> bool {
    *value == T::default()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SliceSettings {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub regions: Vec<SliceDef>,
    #[serde(default, skip_serializing_if = "is_default")]
    pub select: SliceSelect,
    #[serde(default = "default_base_note")]
    pub base_note: u8,
    #[serde(default = "default_seed")]
    pub seed: u32,
}

impl Default for SliceSettings {
    fn default() -> Self {
        Self {
            version: SLICES_VERSION,
            regions: Vec::new(),
            select: SliceSelect::None,
            base_note: default_base_note(),
            seed: default_seed(),
        }
    }
}

impl SliceSettings {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Strict validation: finite in-range regions, at most [`MAX_SLICES`],
    /// no empty and no overlapping regions.
    pub fn validate(&self) -> Result<(), String> {
        if self.version != SLICES_VERSION {
            return Err(format!(
                "unsupported slices version {} (expected {SLICES_VERSION})",
                self.version
            ));
        }
        if self.regions.len() > MAX_SLICES {
            return Err(format!("a sample supports at most {MAX_SLICES} slices"));
        }
        if u16::from(self.base_note) + self.regions.len() as u16 > 128
            && self.select == SliceSelect::Note
        {
            return Err("slice base_note plus slice count exceeds MIDI note 127".into());
        }
        for (index, slice) in self.regions.iter().enumerate() {
            let valid = |value: f64| value.is_finite() && (0.0..=1.0).contains(&value);
            if !valid(slice.start) || !valid(slice.end) || slice.start >= slice.end {
                return Err(format!(
                    "slice {} must satisfy 0.0 <= start < end <= 1.0",
                    index + 1
                ));
            }
        }
        let mut order: Vec<usize> = (0..self.regions.len()).collect();
        order.sort_by(|a, b| self.regions[*a].start.total_cmp(&self.regions[*b].start));
        for pair in order.windows(2) {
            if self.regions[pair[0]].end > self.regions[pair[1]].start {
                return Err(format!(
                    "slices {} and {} overlap",
                    pair[0] + 1,
                    pair[1] + 1
                ));
            }
        }
        Ok(())
    }

    pub fn add(&mut self, slice: SliceDef) -> Result<usize, String> {
        let mut candidate = self.clone();
        candidate.regions.push(slice);
        candidate.validate()?;
        *self = candidate;
        Ok(self.regions.len() - 1)
    }

    pub fn delete(&mut self, index: usize) -> Result<(), String> {
        if index >= self.regions.len() {
            return Err(format!("slice {} does not exist", index + 1));
        }
        self.regions.remove(index);
        Ok(())
    }
}

/// `count` contiguous equal slices covering the whole sample.
pub fn equal_slices(count: usize) -> Result<Vec<SliceDef>, String> {
    if !(1..=MAX_SLICES).contains(&count) {
        return Err(format!("slice count must be between 1 and {MAX_SLICES}"));
    }
    Ok((0..count)
        .map(|index| SliceDef {
            start: index as f64 / count as f64,
            end: (index + 1) as f64 / count as f64,
        })
        .collect())
}

/// Offline transient detection on a decoded sample. `sensitivity` is
/// 0.0 (only strong hits) to 1.0 (many markers). Returns contiguous slices
/// from each detected onset to the next, at most `max_slices`, each at
/// least 20 ms long. Deterministic; never runs on the audio thread.
pub fn detect_slices(
    frames: &[StereoFrame],
    sample_rate: u32,
    max_slices: usize,
    sensitivity: f32,
) -> Result<Vec<SliceDef>, String> {
    if frames.is_empty() {
        return Err("cannot detect slices in an empty sample".into());
    }
    if sample_rate == 0 {
        return Err("sample rate must be greater than zero".into());
    }
    if !sensitivity.is_finite() || !(0.0..=1.0).contains(&sensitivity) {
        return Err("sensitivity must be between 0.0 and 1.0".into());
    }
    let max_slices = max_slices.clamp(1, MAX_SLICES);
    let hop = (sample_rate as usize / 200).max(1); // 5 ms
    let min_gap = (0.020 * f64::from(sample_rate) / hop as f64)
        .ceil()
        .max(1.0) as usize;

    let energies: Vec<f32> = frames
        .chunks(hop)
        .map(|chunk| {
            let sum: f32 = chunk
                .iter()
                .map(|f| f.left * f.left + f.right * f.right)
                .sum();
            (sum / chunk.len() as f32).sqrt()
        })
        .collect();
    let peak = energies.iter().copied().fold(0.0_f32, f32::max);
    if peak <= 1e-6 {
        return Ok(vec![SliceDef {
            start: 0.0,
            end: 1.0,
        }]);
    }

    // Onset strength: rise of the energy envelope above a short running mean.
    let ratio_threshold = 1.5 + (1.0 - sensitivity) * 4.0;
    let floor = peak * (0.02 + (1.0 - sensitivity) * 0.08);
    let mut onsets: Vec<(usize, f32)> = Vec::new();
    let mut last = 0usize;
    let mut have_last = false;
    for index in 1..energies.len() {
        let from = index.saturating_sub(8);
        let mean = energies[from..index].iter().sum::<f32>() / (index - from) as f32;
        let current = energies[index];
        if current > floor
            && current > mean * ratio_threshold + 1e-9
            && (!have_last || index - last >= min_gap)
        {
            onsets.push((index, current / (mean + 1e-9)));
            last = index;
            have_last = true;
        }
    }
    if onsets.len() + 1 > max_slices {
        onsets.sort_by(|a, b| b.1.total_cmp(&a.1));
        onsets.truncate(max_slices.saturating_sub(1));
        onsets.sort_by_key(|(index, _)| *index);
    }

    let len = frames.len() as f64;
    let mut starts: Vec<f64> = vec![0.0];
    for (index, _) in onsets {
        let position = (index * hop) as f64 / len;
        if position > *starts.last().unwrap_or(&0.0) {
            starts.push(position.min(1.0));
        }
    }
    let mut regions = Vec::with_capacity(starts.len());
    for (i, start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(1.0);
        if end > *start {
            regions.push(SliceDef { start: *start, end });
        }
    }
    Ok(regions)
}

/// Fixed-capacity, `Copy` slice table handed to the audio thread so a live
/// slice edit is one bounded message with nothing to free afterwards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliceTable {
    regions: [SliceDef; MAX_SLICES],
    count: usize,
    select: SliceSelect,
    base_note: u8,
    seed: u32,
}

impl Default for SliceTable {
    fn default() -> Self {
        Self {
            regions: [SliceDef {
                start: 0.0,
                end: 1.0,
            }; MAX_SLICES],
            count: 0,
            select: SliceSelect::None,
            base_note: default_base_note(),
            seed: default_seed(),
        }
    }
}

impl SliceTable {
    pub fn compile(settings: &SliceSettings) -> Result<Self, String> {
        settings.validate()?;
        let mut table = Self {
            count: settings.regions.len(),
            select: settings.select,
            base_note: settings.base_note,
            seed: settings.seed,
            ..Self::default()
        };
        table.regions[..settings.regions.len()].copy_from_slice(&settings.regions);
        Ok(table)
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn select(&self) -> SliceSelect {
        self.select
    }

    pub fn base_note(&self) -> u8 {
        self.base_note
    }

    pub fn seed(&self) -> u32 {
        self.seed
    }

    pub fn region(&self, index: usize) -> Option<SliceDef> {
        (index < self.count).then(|| self.regions[index])
    }
}
