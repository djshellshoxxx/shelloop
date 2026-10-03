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

pub fn parse_command(_line: &str) -> Result<Command, String> {
    Err("command parsing not implemented".into())
}
