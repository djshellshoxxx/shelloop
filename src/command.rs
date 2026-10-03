#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Play,
    Pause,
    Stop,
    Restart,
    Panic,
    Tempo(f64),
    Save(String),
    Load(String),
}

pub fn parse_command(line: &str) -> Result<Command, String> {
    let line = line.trim();
    if line.is_empty() {
        return Err("empty command".into());
    }

    let (verb, raw_arg) = line
        .split_once(char::is_whitespace)
        .map(|(verb, rest)| (verb, Some(rest.trim())))
        .unwrap_or((line, None));
    let verb = verb.to_ascii_lowercase();

    match verb.as_str() {
        "play" => no_arg(Command::Play, raw_arg),
        "pause" => no_arg(Command::Pause, raw_arg),
        "stop" => no_arg(Command::Stop, raw_arg),
        "restart" => no_arg(Command::Restart, raw_arg),
        "panic" => no_arg(Command::Panic, raw_arg),
        "tempo" | "bpm" => {
            let arg = require_arg(raw_arg, "tempo requires a BPM value")?;
            let bpm: f64 = arg
                .parse()
                .map_err(|_| format!("invalid BPM value: {arg}"))?;
            if !bpm.is_finite() || !(20.0..=400.0).contains(&bpm) {
                return Err("BPM must be finite and between 20 and 400".into());
            }
            Ok(Command::Tempo(bpm))
        }
        "save" => Ok(Command::Save(parse_path_arg(require_arg(
            raw_arg,
            "save requires a path",
        )?)?)),
        "load" => Ok(Command::Load(parse_path_arg(require_arg(
            raw_arg,
            "load requires a path",
        )?)?)),
        _ => Err(format!("unknown command: {verb}")),
    }
}

fn no_arg(command: Command, raw_arg: Option<&str>) -> Result<Command, String> {
    if raw_arg.is_some_and(|arg| !arg.is_empty()) {
        return Err("command does not accept arguments".into());
    }
    Ok(command)
}

fn require_arg<'a>(raw_arg: Option<&'a str>, message: &str) -> Result<&'a str, String> {
    raw_arg.filter(|arg| !arg.is_empty()).ok_or_else(|| message.into())
}

fn parse_path_arg(arg: &str) -> Result<String, String> {
    let arg = arg.trim();
    if arg.starts_with('"') || arg.ends_with('"') {
        if arg.len() < 2 || !arg.starts_with('"') || !arg.ends_with('"') {
            return Err("unterminated quoted path".into());
        }
        let inner = &arg[1..arg.len() - 1];
        if inner.is_empty() {
            return Err("path may not be empty".into());
        }
        return Ok(inner.to_owned());
    }
    Ok(arg.to_owned())
}
