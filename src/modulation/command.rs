//! Text commands for the modulation matrix (`mod ...`), parsed outside the
//! audio thread and applied to a [`ModulationConfig`] snapshot.
//!
//! ```text
//! mod add <source> <dest> <amount> [smoothing_ms]
//! mod set <n> <source> <dest> <amount> [smoothing_ms]
//! mod remove <n>      mod on <n>      mod off <n>
//! mod clear           mod list
//! ```
//! Route numbers are one-based.

use super::model::{parse_mod_dest, parse_mod_source, ModRoute, ModulationConfig};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ModCommand {
    Add(ModRoute),
    Set(usize, ModRoute),
    Remove(usize),
    Enable(usize, bool),
    Clear,
    List,
}

fn parse_index(word: Option<&&str>) -> Result<usize, String> {
    word.ok_or("missing route number")?
        .parse::<usize>()
        .ok()
        .filter(|n| *n >= 1)
        .map(|n| n - 1)
        .ok_or_else(|| "route number must be a positive integer".to_owned())
}

fn parse_route(words: &[&str]) -> Result<ModRoute, String> {
    let [source, dest, amount, rest @ ..] = words else {
        return Err("expected <source> <dest> <amount> [smoothing_ms]".into());
    };
    let source = parse_mod_source(source).ok_or_else(|| format!("unknown mod source: {source}"))?;
    let dest = parse_mod_dest(dest).ok_or_else(|| format!("unknown mod destination: {dest}"))?;
    let amount = amount
        .parse::<f32>()
        .map_err(|_| "modulation amount must be numeric".to_owned())?;
    let mut route = ModRoute::new(source, dest, amount);
    match rest {
        [] => {}
        [smoothing] => {
            route.smoothing_ms = smoothing
                .parse::<f32>()
                .map_err(|_| "smoothing must be numeric milliseconds".to_owned())?;
        }
        _ => return Err("too many arguments".into()),
    }
    route.validate()?;
    Ok(route)
}

pub fn parse_mod_command(line: &str) -> Result<ModCommand, String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.first() != Some(&"mod") {
        return Err("expected mod <add|set|remove|on|off|clear|list> ...".into());
    }
    match words.get(1).copied() {
        Some("add") => Ok(ModCommand::Add(parse_route(&words[2..])?)),
        Some("set") => {
            let index = parse_index(words.get(2))?;
            Ok(ModCommand::Set(index, parse_route(&words[3..])?))
        }
        Some("remove") => Ok(ModCommand::Remove(parse_index(words.get(2))?)),
        Some("on") => Ok(ModCommand::Enable(parse_index(words.get(2))?, true)),
        Some("off") => Ok(ModCommand::Enable(parse_index(words.get(2))?, false)),
        Some("clear") => Ok(ModCommand::Clear),
        Some("list") | None => Ok(ModCommand::List),
        Some(other) => Err(format!("unknown mod subcommand: {other}")),
    }
}

pub fn describe_routes(config: &ModulationConfig) -> String {
    if config.routes().is_empty() {
        return "no modulation routes".to_owned();
    }
    config
        .routes()
        .iter()
        .enumerate()
        .map(|(index, route)| {
            format!(
                "{}: {:?} -> {:?} {:+.2}{}{}",
                index + 1,
                route.source,
                route.dest,
                route.amount,
                if route.smoothing_ms > 0.0 {
                    format!(" smooth {}ms", route.smoothing_ms)
                } else {
                    String::new()
                },
                if route.enabled { "" } else { " (off)" }
            )
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Applies a command to `config`; on error `config` is left unchanged.
pub fn apply_mod_command(
    config: &mut ModulationConfig,
    command: ModCommand,
) -> Result<String, String> {
    match command {
        ModCommand::Add(route) => {
            let index = config.add_route(route)?;
            Ok(format!("added modulation route {}", index + 1))
        }
        ModCommand::Set(index, route) => {
            config.set_route(index, route)?;
            Ok(format!("updated modulation route {}", index + 1))
        }
        ModCommand::Remove(index) => {
            config.remove_route(index)?;
            Ok(format!("removed modulation route {}", index + 1))
        }
        ModCommand::Enable(index, enabled) => {
            let mut route = *config
                .routes()
                .get(index)
                .ok_or_else(|| format!("modulation route {} does not exist", index + 1))?;
            route.enabled = enabled;
            config.set_route(index, route)?;
            Ok(format!(
                "modulation route {} {}",
                index + 1,
                if enabled { "on" } else { "off" }
            ))
        }
        ModCommand::Clear => {
            config.clear_routes();
            Ok("cleared modulation routes".to_owned())
        }
        ModCommand::List => Ok(describe_routes(config)),
    }
}

/// Human-readable bound of the destination while only `route` is active at
/// full source scale, for inspector feedback. The live value always lies in
/// this range; it is a bound, not a measurement.
pub fn route_range_text(patch: &crate::SynthPatch, route: &super::model::ModRoute) -> String {
    use super::model::{ModDest, ModSource};
    let depth = route.amount.abs();
    let bipolar = matches!(
        route.source,
        ModSource::Lfo1 | ModSource::Lfo2 | ModSource::KeyTrack
    );
    let span = |scale: f32| (if bipolar { -scale } else { 0.0_f32.min(scale) }, scale);
    match route.dest {
        ModDest::Tuning => format!("+-{:.1} st", 12.0 * depth),
        ModDest::PulseWidth => {
            let (lo, hi) = span(0.45 * depth);
            format!(
                "{:.2}..{:.2}",
                (patch.pulse_width + lo).clamp(0.05, 0.95),
                (patch.pulse_width + hi).clamp(0.05, 0.95)
            )
        }
        ModDest::FilterCutoff => {
            let (lo, hi) = span(4.0 * depth);
            let base = patch.filter.cutoff_hz;
            format!(
                "{:.0}..{:.0} Hz",
                (base * lo.exp2()).clamp(20.0, 20_000.0),
                (base * hi.exp2()).clamp(20.0, 20_000.0)
            )
        }
        ModDest::FilterResonance => {
            let (lo, hi) = span(depth);
            format!(
                "{:.2}..{:.2}",
                (patch.filter.resonance + lo).clamp(0.0, 1.0),
                (patch.filter.resonance + hi).clamp(0.0, 1.0)
            )
        }
        ModDest::Amplitude => {
            let (lo, hi) = span(depth);
            format!(
                "x{:.2}..x{:.2}",
                (1.0 + lo).clamp(0.0, 2.0),
                (1.0 + hi).clamp(0.0, 2.0)
            )
        }
        ModDest::Pan => {
            let (lo, hi) = span(depth);
            format!("{:+.2}..{:+.2}", lo.clamp(-1.0, 1.0), hi.clamp(-1.0, 1.0))
        }
        ModDest::EffectWet => {
            let (lo, hi) = span(depth);
            format!(
                "{:+.2}..{:+.2} send",
                lo.clamp(-1.0, 1.0),
                hi.clamp(-1.0, 1.0)
            )
        }
    }
}
