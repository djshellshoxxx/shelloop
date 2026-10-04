use crate::MidiPortSelector;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupOptions {
    pub audio_device: Option<String>,
    pub midi_port: Option<MidiPortSelector>,
    pub polyphony: usize,
    pub list_devices: bool,
    pub no_midi: bool,
    pub show_help: bool,
}

impl Default for StartupOptions {
    fn default() -> Self {
        Self {
            audio_device: None,
            midi_port: Some(MidiPortSelector::First),
            polyphony: 16,
            list_devices: false,
            no_midi: false,
            show_help: false,
        }
    }
}

pub fn parse_startup_options<I, S>(args: I) -> Result<StartupOptions, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let args: Vec<String> = args
        .into_iter()
        .map(|value| value.as_ref().to_string())
        .collect();
    let mut options = StartupOptions::default();
    let mut index = 0;
    let mut explicit_midi = false;

    while index < args.len() {
        match args[index].as_str() {
            "--audio-device" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--audio-device requires a device name".to_string())?;
                if value.trim().is_empty() {
                    return Err("--audio-device requires a non-empty device name".into());
                }
                options.audio_device = Some(value.clone());
            }
            "--midi-name" => {
                if explicit_midi {
                    return Err("choose only one MIDI selector".into());
                }
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--midi-name requires a port name".to_string())?;
                if value.trim().is_empty() {
                    return Err("--midi-name requires a non-empty port name".into());
                }
                options.midi_port = Some(MidiPortSelector::Name(value.clone()));
                explicit_midi = true;
            }
            "--midi-index" => {
                if explicit_midi {
                    return Err("choose only one MIDI selector".into());
                }
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--midi-index requires a zero-based index".to_string())?;
                let port_index = value
                    .parse::<usize>()
                    .map_err(|_| "--midi-index must be a non-negative integer".to_string())?;
                options.midi_port = Some(MidiPortSelector::Index(port_index));
                explicit_midi = true;
            }
            "--polyphony" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--polyphony requires a voice count".to_string())?;
                let polyphony = value
                    .parse::<usize>()
                    .map_err(|_| "--polyphony must be an integer from 1 to 256".to_string())?;
                if !(1..=256).contains(&polyphony) {
                    return Err("--polyphony must be between 1 and 256".into());
                }
                options.polyphony = polyphony;
            }
            "--list-devices" => options.list_devices = true,
            "--no-midi" => options.no_midi = true,
            "--help" | "-h" => options.show_help = true,
            unknown => return Err(format!("unknown option: {unknown}")),
        }
        index += 1;
    }

    if options.no_midi && explicit_midi {
        return Err("--no-midi cannot be combined with a MIDI port selector".into());
    }

    Ok(options)
}
