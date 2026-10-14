//! Text commands for sample shaping and slices.
//!
//! ```text
//! sample filter <off|lowpass|highpass> [cutoff_hz] [resonance]
//! sample drive <db>
//! sample envelope <attack> <decay> <sustain> <release> | sample envelope off
//! sample transient <attack> <sustain>
//! sample undo
//! slice list
//! slice add <start> <end>          (normalised 0..1)
//! slice delete <n>                 (one-based)
//! slice auto <count>               (equal division)
//! slice assign <none|note|random> [base_note|seed]
//! slice clear
//! ```

use super::shaping::{SampleEnvelope, SampleFilterMode, SampleShaping};
use super::slices::{equal_slices, SliceDef, SliceSelect, SliceSettings};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SampleCommand {
    Filter {
        mode: SampleFilterMode,
        cutoff: Option<f32>,
        resonance: Option<f32>,
    },
    Drive(f32),
    Envelope(Option<SampleEnvelope>),
    Transient {
        attack: f32,
        sustain: f32,
    },
    SliceList,
    SliceAdd(SliceDef),
    SliceDelete(usize),
    SliceAuto(usize),
    SliceAssign {
        select: SliceSelect,
        value: Option<u32>,
    },
    SliceClear,
    Undo,
}

fn number(word: Option<&&str>, what: &str) -> Result<f32, String> {
    word.ok_or_else(|| format!("missing {what}"))?
        .parse::<f32>()
        .ok()
        .filter(|v| v.is_finite())
        .ok_or_else(|| format!("{what} must be a finite number"))
}

pub fn parse_sample_command(line: &str) -> Result<SampleCommand, String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    match (words.first().copied(), words.get(1).copied()) {
        (Some("sample"), Some("filter")) => {
            let mode = match words.get(2).copied() {
                Some("off") => SampleFilterMode::Off,
                Some("lowpass") | Some("lp") => SampleFilterMode::LowPass,
                Some("highpass") | Some("hp") => SampleFilterMode::HighPass,
                _ => {
                    return Err(
                        "expected sample filter <off|lowpass|highpass> [cutoff] [resonance]".into(),
                    )
                }
            };
            let cutoff = words
                .get(3)
                .map(|_| number(words.get(3), "cutoff"))
                .transpose()?;
            let resonance = words
                .get(4)
                .map(|_| number(words.get(4), "resonance"))
                .transpose()?;
            Ok(SampleCommand::Filter {
                mode,
                cutoff,
                resonance,
            })
        }
        (Some("sample"), Some("drive")) => {
            Ok(SampleCommand::Drive(number(words.get(2), "drive dB")?))
        }
        (Some("sample"), Some("envelope")) => {
            if words.get(2) == Some(&"off") {
                return Ok(SampleCommand::Envelope(None));
            }
            Ok(SampleCommand::Envelope(Some(SampleEnvelope {
                attack_secs: number(words.get(2), "attack")?,
                decay_secs: number(words.get(3), "decay")?,
                sustain: number(words.get(4), "sustain")?,
                release_secs: number(words.get(5), "release")?,
            })))
        }
        (Some("sample"), Some("transient")) => Ok(SampleCommand::Transient {
            attack: number(words.get(2), "transient attack")?,
            sustain: number(words.get(3), "transient sustain")?,
        }),
        (Some("sample"), Some("undo")) => Ok(SampleCommand::Undo),
        (Some("slice"), Some("list")) | (Some("slice"), None) => Ok(SampleCommand::SliceList),
        (Some("slice"), Some("add")) => Ok(SampleCommand::SliceAdd(SliceDef {
            start: f64::from(number(words.get(2), "slice start")?),
            end: f64::from(number(words.get(3), "slice end")?),
        })),
        (Some("slice"), Some("delete")) => {
            let n = number(words.get(2), "slice number")?;
            if n < 1.0 || n.fract() != 0.0 {
                return Err("slice number must be a positive integer".into());
            }
            Ok(SampleCommand::SliceDelete(n as usize - 1))
        }
        (Some("slice"), Some("auto")) => {
            let n = number(words.get(2), "slice count")?;
            if n < 1.0 || n.fract() != 0.0 {
                return Err("slice count must be a positive integer".into());
            }
            Ok(SampleCommand::SliceAuto(n as usize))
        }
        (Some("slice"), Some("assign")) => {
            let select = match words.get(2).copied() {
                Some("none") => SliceSelect::None,
                Some("note") => SliceSelect::Note,
                Some("random") => SliceSelect::Random,
                _ => return Err("expected slice assign <none|note|random> [value]".into()),
            };
            let value = words
                .get(3)
                .map(|_| number(words.get(3), "value").map(|v| v as u32))
                .transpose()?;
            Ok(SampleCommand::SliceAssign { select, value })
        }
        (Some("slice"), Some("clear")) => Ok(SampleCommand::SliceClear),
        (Some("sample"), _) => {
            Err("expected sample <filter|drive|envelope|transient|undo> ...".into())
        }
        (Some("slice"), Some(other)) => Err(format!("unknown slice subcommand: {other}")),
        _ => Err("expected a sample or slice command".into()),
    }
}

/// Applies `command` to copies and commits only if the result validates.
/// `Undo` and `SliceList` are handled by the caller.
pub fn apply_sample_command(
    shaping: &mut SampleShaping,
    slices: &mut SliceSettings,
    command: SampleCommand,
) -> Result<String, String> {
    let mut next_shaping = *shaping;
    let mut next_slices = slices.clone();
    let message = match command {
        SampleCommand::Filter {
            mode,
            cutoff,
            resonance,
        } => {
            next_shaping.filter_mode = mode;
            if let Some(cutoff) = cutoff {
                next_shaping.filter_cutoff = cutoff;
            }
            if let Some(resonance) = resonance {
                next_shaping.filter_resonance = resonance;
            }
            format!("sample filter {mode:?}")
        }
        SampleCommand::Drive(db) => {
            next_shaping.drive_db = db;
            format!("sample drive {db} dB")
        }
        SampleCommand::Envelope(envelope) => {
            next_shaping.envelope = envelope;
            if envelope.is_some() {
                "sample envelope on"
            } else {
                "sample envelope off"
            }
            .to_owned()
        }
        SampleCommand::Transient { attack, sustain } => {
            next_shaping.transient_attack = attack;
            next_shaping.transient_sustain = sustain;
            "sample transient shaper updated".to_owned()
        }
        SampleCommand::SliceAdd(def) => {
            let index = next_slices.add(def)?;
            format!("added slice {}", index + 1)
        }
        SampleCommand::SliceDelete(index) => {
            next_slices.delete(index)?;
            format!("deleted slice {}", index + 1)
        }
        SampleCommand::SliceAuto(count) => {
            next_slices.regions = equal_slices(count)?;
            format!("created {count} equal slices")
        }
        SampleCommand::SliceAssign { select, value } => {
            next_slices.select = select;
            if let Some(value) = value {
                match select {
                    SliceSelect::Note => {
                        next_slices.base_note = u8::try_from(value)
                            .ok()
                            .filter(|n| *n <= 127)
                            .ok_or("base note must be 0..=127")?;
                    }
                    SliceSelect::Random => next_slices.seed = value,
                    SliceSelect::None => {}
                }
            }
            format!("slice assignment {select:?}")
        }
        SampleCommand::SliceClear => {
            next_slices.regions.clear();
            "cleared slices".to_owned()
        }
        SampleCommand::SliceList | SampleCommand::Undo => {
            return Err("handled by the caller".into());
        }
    };
    next_shaping.validate()?;
    next_slices.validate()?;
    *shaping = next_shaping;
    *slices = next_slices;
    Ok(message)
}

pub fn describe_slices(slices: &SliceSettings) -> String {
    if slices.regions.is_empty() {
        return "no slices".to_owned();
    }
    let list = slices
        .regions
        .iter()
        .enumerate()
        .map(|(i, s)| format!("{}: {:.3}-{:.3}", i + 1, s.start, s.end))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "{:?} base {} seed {} | {list}",
        slices.select, slices.base_note, slices.seed
    )
}
