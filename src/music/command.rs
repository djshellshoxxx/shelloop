//! Text syntax for the musical commands. Parsing only; the controller applies
//! the parsed command. Step numbers are one-based, as in `step N ...`.

use crate::music::arp::ArpMode;
use crate::music::chord::ChordQuality;
use crate::music::humanize::MAX_HUMANIZE_VELOCITY;
use crate::music::ratio::parse_ratio;
use crate::music::scale::{parse_note, parse_pitch_class, ScaleKind};
use crate::music::MAX_EUCLID_STEPS;

/// Largest transposition accepted by `pattern transpose`.
pub const MAX_TRANSPOSE_SEMITONES: i32 = 48;
/// Largest humanize timing jitter accepted in milliseconds.
pub const MAX_HUMANIZE_MS: f64 = 200.0;
/// Largest count-in, in bars.
pub const MAX_COUNT_IN_BARS: u32 = 4;
const DEFAULT_EUCLID_NOTE: u8 = 60;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MusicCommand {
    ScaleStatus,
    ScaleList,
    ScaleOff,
    ScaleSnap(bool),
    ScaleSet {
        root: u8,
        kind: ScaleKind,
    },
    ScaleApply,
    Chord {
        root: u8,
        quality: ChordQuality,
        inversion: u8,
    },
    Arp {
        root: u8,
        quality: ChordQuality,
        mode: ArpMode,
        octaves: u8,
        rate: usize,
    },
    Euclid {
        hits: usize,
        steps: usize,
        rotation: usize,
        note: u8,
    },
    /// `None` turns humanize off.
    Humanize(Option<(f64, f32)>),
    /// Zero-based step and its ratio, `None` to clear.
    Ratio {
        step: usize,
        ratio: Option<(u8, u8)>,
    },
    Tap,
    Metronome {
        enabled: bool,
        gain: f32,
    },
    CountIn(u32),
    AutosaveStatus,
    AutosaveEnabled(bool),
    AutosaveEvery(u64),
    PatternCopy,
    PatternPaste,
    PatternClear,
    PatternTranspose(i32),
}

/// Parse one musical command line. The first word selects the command;
/// `pattern` takes the `copy|paste|clear|transpose` subcommands only.
pub fn parse_music_command(line: &str) -> Result<MusicCommand, String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some((&head, args)) = words.split_first() else {
        return Err("empty command".into());
    };
    match head {
        "scale" => parse_scale(args),
        "chord" => parse_chord(args),
        "arp" => parse_arp(args),
        "euclid" => parse_euclid(args),
        "humanize" => parse_humanize(args),
        "ratio" => parse_ratio_command(args),
        "tap" => no_args(args, MusicCommand::Tap, "tap"),
        "metronome" => parse_metronome(args),
        "countin" => parse_countin(args),
        "autosave" => parse_autosave(args),
        "pattern" => parse_pattern_op(args),
        other => Err(format!("unknown musical command: {other}")),
    }
}

fn no_args(args: &[&str], command: MusicCommand, name: &str) -> Result<MusicCommand, String> {
    if args.is_empty() {
        Ok(command)
    } else {
        Err(format!("{name} does not accept arguments"))
    }
}

fn parse_scale(args: &[&str]) -> Result<MusicCommand, String> {
    match args {
        [] => Ok(MusicCommand::ScaleStatus),
        ["list"] => Ok(MusicCommand::ScaleList),
        ["off"] => Ok(MusicCommand::ScaleOff),
        ["apply"] => Ok(MusicCommand::ScaleApply),
        ["snap", state] => Ok(MusicCommand::ScaleSnap(parse_on_off(state)?)),
        [root, kind] => Ok(MusicCommand::ScaleSet {
            root: parse_pitch_class(root)?,
            kind: ScaleKind::parse(kind)?,
        }),
        _ => Err(
            "scale: use scale list | scale ROOT KIND | scale off | scale snap on|off | scale apply"
                .into(),
        ),
    }
}

fn parse_chord(args: &[&str]) -> Result<MusicCommand, String> {
    let (root, quality, inversion) = match args {
        [root, quality] => (*root, *quality, "0"),
        [root, quality, inversion] => (*root, *quality, *inversion),
        _ => return Err("chord: use chord ROOT QUALITY [INVERSION], e.g. chord C4 min 1".into()),
    };
    Ok(MusicCommand::Chord {
        root: parse_note(root)?,
        quality: ChordQuality::parse(quality)?,
        inversion: parse_small(inversion, "inversion", 0..=3)? as u8,
    })
}

fn parse_arp(args: &[&str]) -> Result<MusicCommand, String> {
    if !(2..=5).contains(&args.len()) {
        return Err("arp: use arp ROOT QUALITY [MODE] [OCTAVES] [RATE]".into());
    }
    let mode = match args.get(2) {
        Some(mode) => ArpMode::parse(mode)?,
        None => ArpMode::Up,
    };
    let octaves = match args.get(3) {
        Some(value) => parse_small(value, "octaves", 1..=4)? as u8,
        None => 1,
    };
    let rate = match args.get(4) {
        Some(value) => parse_small(value, "rate", 1..=16)? as usize,
        None => 1,
    };
    Ok(MusicCommand::Arp {
        root: parse_note(args[0])?,
        quality: ChordQuality::parse(args[1])?,
        mode,
        octaves,
        rate,
    })
}

fn parse_euclid(args: &[&str]) -> Result<MusicCommand, String> {
    if args.len() < 2 {
        return Err("euclid: use euclid HITS STEPS [rotate N] [note NOTE]".into());
    }
    let hits = parse_small(args[0], "hits", 0..=MAX_EUCLID_STEPS as u64)? as usize;
    let steps = parse_small(args[1], "steps", 1..=MAX_EUCLID_STEPS as u64)? as usize;
    let mut rotation = 0_usize;
    let mut note = DEFAULT_EUCLID_NOTE;
    let mut rest = &args[2..];
    while let [option, value, tail @ ..] = rest {
        match *option {
            "rotate" => {
                rotation = parse_small(value, "rotate", 0..=MAX_EUCLID_STEPS as u64)? as usize
            }
            "note" => note = parse_note(value)?,
            other => return Err(format!("euclid: unknown option {other}")),
        }
        rest = tail;
    }
    if !rest.is_empty() {
        return Err("euclid: options come in pairs, e.g. rotate 2".into());
    }
    if hits > steps {
        return Err("euclid: hits may not exceed steps".into());
    }
    Ok(MusicCommand::Euclid {
        hits,
        steps,
        rotation,
        note,
    })
}

fn parse_humanize(args: &[&str]) -> Result<MusicCommand, String> {
    match args {
        ["off"] => Ok(MusicCommand::Humanize(None)),
        [timing, velocity] => {
            let timing: f64 = timing
                .parse()
                .map_err(|_| format!("invalid humanize timing: {timing}"))?;
            if !timing.is_finite() || !(0.0..=MAX_HUMANIZE_MS).contains(&timing) {
                return Err(format!("humanize timing must be 0..={MAX_HUMANIZE_MS} ms"));
            }
            let velocity: f32 = velocity
                .parse()
                .map_err(|_| format!("invalid humanize velocity: {velocity}"))?;
            if !velocity.is_finite() || !(0.0..=MAX_HUMANIZE_VELOCITY).contains(&velocity) {
                return Err(format!(
                    "humanize velocity must be 0..={MAX_HUMANIZE_VELOCITY}"
                ));
            }
            Ok(MusicCommand::Humanize(Some((timing, velocity))))
        }
        _ => Err("humanize: use humanize MS VELOCITY or humanize off".into()),
    }
}

fn parse_ratio_command(args: &[&str]) -> Result<MusicCommand, String> {
    let [step, condition] = args else {
        return Err("ratio: use ratio STEP P:Q (e.g. ratio 5 1:2) or ratio STEP off".into());
    };
    let step = parse_small(step, "step", 1..=256)? as usize - 1;
    let ratio = if *condition == "off" {
        None
    } else {
        Some(parse_ratio(condition)?)
    };
    Ok(MusicCommand::Ratio { step, ratio })
}

fn parse_metronome(args: &[&str]) -> Result<MusicCommand, String> {
    match args {
        ["off"] => Ok(MusicCommand::Metronome {
            enabled: false,
            gain: 0.5,
        }),
        ["on"] => Ok(MusicCommand::Metronome {
            enabled: true,
            gain: 0.5,
        }),
        ["on", gain] => {
            let gain: f32 = gain
                .parse()
                .map_err(|_| format!("invalid metronome gain: {gain}"))?;
            if !gain.is_finite() || !(0.0..=1.0).contains(&gain) {
                return Err("metronome gain must be between 0.0 and 1.0".into());
            }
            Ok(MusicCommand::Metronome {
                enabled: true,
                gain,
            })
        }
        _ => Err("metronome: use metronome on [GAIN] | metronome off".into()),
    }
}

fn parse_countin(args: &[&str]) -> Result<MusicCommand, String> {
    let [bars] = args else {
        return Err(format!(
            "countin: use countin BARS (0..={MAX_COUNT_IN_BARS}, 0 turns it off)"
        ));
    };
    Ok(MusicCommand::CountIn(
        parse_small(bars, "bars", 0..=u64::from(MAX_COUNT_IN_BARS))? as u32,
    ))
}

fn parse_autosave(args: &[&str]) -> Result<MusicCommand, String> {
    match args {
        [] => Ok(MusicCommand::AutosaveStatus),
        ["on"] => Ok(MusicCommand::AutosaveEnabled(true)),
        ["off"] => Ok(MusicCommand::AutosaveEnabled(false)),
        ["every", seconds] => Ok(MusicCommand::AutosaveEvery(
            parse_small(seconds, "seconds", 1..=86_400)? * 1000,
        )),
        _ => Err("autosave: use autosave on|off | autosave every SECONDS".into()),
    }
}

fn parse_pattern_op(args: &[&str]) -> Result<MusicCommand, String> {
    match args {
        ["copy"] => Ok(MusicCommand::PatternCopy),
        ["paste"] => Ok(MusicCommand::PatternPaste),
        ["clear"] => Ok(MusicCommand::PatternClear),
        ["transpose", semitones] => {
            let semitones: i32 = semitones
                .parse()
                .map_err(|_| format!("invalid transpose amount: {semitones}"))?;
            if semitones.abs() > MAX_TRANSPOSE_SEMITONES {
                return Err(format!(
                    "transpose must be within ±{MAX_TRANSPOSE_SEMITONES}"
                ));
            }
            Ok(MusicCommand::PatternTranspose(semitones))
        }
        _ => Err("pattern: use pattern copy|paste|clear|transpose N".into()),
    }
}

fn parse_on_off(text: &str) -> Result<bool, String> {
    match text {
        "on" => Ok(true),
        "off" => Ok(false),
        other => Err(format!("expected on or off, got {other}")),
    }
}

fn parse_small(
    text: &str,
    label: &str,
    range: std::ops::RangeInclusive<u64>,
) -> Result<u64, String> {
    let value: u64 = text
        .parse()
        .map_err(|_| format!("invalid {label}: {text}"))?;
    if !range.contains(&value) {
        return Err(format!(
            "{label} must be between {} and {}",
            range.start(),
            range.end()
        ));
    }
    Ok(value)
}

/// Words that route `pattern ...` to the musical parser rather than the
/// existing `pattern <id>` selection.
pub fn is_pattern_operation(word: Option<&str>) -> bool {
    matches!(word, Some("copy" | "paste" | "clear" | "transpose"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_forms() {
        assert_eq!(
            parse_music_command("scale").unwrap(),
            MusicCommand::ScaleStatus
        );
        assert_eq!(
            parse_music_command("scale off").unwrap(),
            MusicCommand::ScaleOff
        );
        assert_eq!(
            parse_music_command("scale snap off").unwrap(),
            MusicCommand::ScaleSnap(false)
        );
        assert_eq!(
            parse_music_command("scale Eb minor").unwrap(),
            MusicCommand::ScaleSet {
                root: 3,
                kind: ScaleKind::Minor
            }
        );
        assert!(parse_music_command("scale H minor").is_err());
        assert!(parse_music_command("scale C klingon").is_err());
    }

    #[test]
    fn chord_and_arp_forms() {
        assert_eq!(
            parse_music_command("chord C4 min 1").unwrap(),
            MusicCommand::Chord {
                root: 60,
                quality: ChordQuality::Minor,
                inversion: 1
            }
        );
        assert_eq!(
            parse_music_command("arp A3 min7 updown 2 4").unwrap(),
            MusicCommand::Arp {
                root: 57,
                quality: ChordQuality::Minor7,
                mode: ArpMode::UpDown,
                octaves: 2,
                rate: 4,
            }
        );
        assert!(parse_music_command("arp C4 maj random 5").is_err());
        assert!(parse_music_command("arp C4").is_err());
        assert!(parse_music_command("chord C4 maj 4").is_err());
        // A triad parses; the inversion is rejected when the chord is built.
        assert!(parse_music_command("chord C4 maj 3").is_ok());
    }

    #[test]
    fn euclid_options() {
        assert_eq!(
            parse_music_command("euclid 5 8 rotate 2 note 36").unwrap(),
            MusicCommand::Euclid {
                hits: 5,
                steps: 8,
                rotation: 2,
                note: 36
            }
        );
        assert!(parse_music_command("euclid 9 8").is_err());
        assert!(parse_music_command("euclid 3 8 rotate").is_err());
        assert!(parse_music_command("euclid 3 8 swing 2").is_err());
    }

    #[test]
    fn humanize_ratio_and_pattern_ops() {
        assert_eq!(
            parse_music_command("humanize 12 0.1").unwrap(),
            MusicCommand::Humanize(Some((12.0, 0.1)))
        );
        assert_eq!(
            parse_music_command("humanize off").unwrap(),
            MusicCommand::Humanize(None)
        );
        assert!(parse_music_command("humanize 300 0.1").is_err());
        assert_eq!(
            parse_music_command("ratio 5 1:2").unwrap(),
            MusicCommand::Ratio {
                step: 4,
                ratio: Some((1, 2))
            }
        );
        assert_eq!(
            parse_music_command("ratio 1 off").unwrap(),
            MusicCommand::Ratio {
                step: 0,
                ratio: None
            }
        );
        assert!(parse_music_command("ratio 0 1:2").is_err());
        assert_eq!(
            parse_music_command("pattern transpose -3").unwrap(),
            MusicCommand::PatternTranspose(-3)
        );
        assert!(parse_music_command("pattern transpose 49").is_err());
        assert!(is_pattern_operation(Some("copy")));
        assert!(!is_pattern_operation(Some("7")));
    }

    #[test]
    fn transport_related_forms() {
        assert_eq!(parse_music_command("tap").unwrap(), MusicCommand::Tap);
        assert!(parse_music_command("tap 3").is_err());
        assert_eq!(
            parse_music_command("metronome on 0.3").unwrap(),
            MusicCommand::Metronome {
                enabled: true,
                gain: 0.3
            }
        );
        assert_eq!(
            parse_music_command("countin 2").unwrap(),
            MusicCommand::CountIn(2)
        );
        assert!(parse_music_command("countin 5").is_err());
        assert_eq!(
            parse_music_command("autosave every 30").unwrap(),
            MusicCommand::AutosaveEvery(30_000)
        );
        assert!(parse_music_command("autosave every 0").is_err());
    }
}
