use crate::MidiPortSelector;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupOptions {
    pub audio_device: Option<String>,
    pub midi_port: Option<MidiPortSelector>,
    pub polyphony: usize,
    pub pattern_path: Option<String>,
    pub project_path: Option<String>,
    pub record_path: Option<String>,
    pub mouse_xy: bool,
    pub bpm: u16,
    pub steps_per_beat: u8,
    pub list_devices: bool,
    pub no_midi: bool,
    pub show_help: bool,
    pub show_version: bool,
    pub pc_speaker_test: bool,
    pub pc_speaker_frequency_hz: u32,
    pub pc_speaker_duration_ms: u32,
}

impl Default for StartupOptions {
    fn default() -> Self {
        Self {
            audio_device: None,
            midi_port: Some(MidiPortSelector::First),
            polyphony: 16,
            pattern_path: None,
            project_path: None,
            record_path: None,
            mouse_xy: false,
            bpm: 120,
            steps_per_beat: 4,
            list_devices: false,
            no_midi: false,
            show_help: false,
            show_version: false,
            pc_speaker_test: false,
            pc_speaker_frequency_hz: 440,
            pc_speaker_duration_ms: 250,
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
    let mut explicit_pc_speaker_frequency = false;
    let mut explicit_pc_speaker_duration = false;

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
            "--pattern" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--pattern requires a JSON file path".to_string())?;
                if value.trim().is_empty() {
                    return Err("--pattern requires a non-empty JSON file path".into());
                }
                options.pattern_path = Some(value.clone());
            }
            "--project" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--project requires a JSON project file path".to_string())?;
                if value.trim().is_empty() {
                    return Err("--project requires a non-empty JSON project file path".into());
                }
                options.project_path = Some(value.clone());
            }
            "--record" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--record requires a WAV file path".to_string())?;
                if value.trim().is_empty() {
                    return Err("--record requires a non-empty WAV file path".into());
                }
                options.record_path = Some(value.clone());
            }
            "--bpm" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--bpm requires an integer tempo".to_string())?;
                let bpm = value
                    .parse::<u16>()
                    .map_err(|_| "--bpm must be an integer from 20 to 400".to_string())?;
                if !(20..=400).contains(&bpm) {
                    return Err("--bpm must be between 20 and 400".into());
                }
                options.bpm = bpm;
            }
            "--steps-per-beat" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--steps-per-beat requires an integer".to_string())?;
                let steps_per_beat = value
                    .parse::<u8>()
                    .map_err(|_| "--steps-per-beat must be an integer from 1 to 64".to_string())?;
                if !(1..=64).contains(&steps_per_beat) {
                    return Err("--steps-per-beat must be between 1 and 64".into());
                }
                options.steps_per_beat = steps_per_beat;
            }
            "--pc-speaker-test" => options.pc_speaker_test = true,
            "--pc-speaker-frequency" => {
                index += 1;
                let value = args.get(index).ok_or_else(|| {
                    "--pc-speaker-frequency requires a frequency in Hz".to_string()
                })?;
                let frequency = value.parse::<u32>().map_err(|_| {
                    "--pc-speaker-frequency must be an integer from 37 to 32767".to_string()
                })?;
                if !(37..=32_767).contains(&frequency) {
                    return Err("--pc-speaker-frequency must be between 37 and 32767 Hz".into());
                }
                options.pc_speaker_frequency_hz = frequency;
                explicit_pc_speaker_frequency = true;
            }
            "--pc-speaker-duration" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--pc-speaker-duration requires milliseconds".to_string())?;
                let duration = value.parse::<u32>().map_err(|_| {
                    "--pc-speaker-duration must be an integer from 1 to 5000".to_string()
                })?;
                if !(1..=5_000).contains(&duration) {
                    return Err("--pc-speaker-duration must be between 1 and 5000 ms".into());
                }
                options.pc_speaker_duration_ms = duration;
                explicit_pc_speaker_duration = true;
            }
            "--list-devices" => options.list_devices = true,
            "--mouse-xy" => options.mouse_xy = true,
            "--no-midi" => options.no_midi = true,
            "--help" | "-h" => options.show_help = true,
            "--version" | "-V" => options.show_version = true,
            unknown => return Err(format!("unknown option: {unknown}")),
        }
        index += 1;
    }

    if options.pattern_path.is_some() && options.project_path.is_some() {
        return Err("--pattern cannot be combined with --project".into());
    }
    if options.no_midi && explicit_midi {
        return Err("--no-midi cannot be combined with a MIDI port selector".into());
    }
    if !options.pc_speaker_test && (explicit_pc_speaker_frequency || explicit_pc_speaker_duration) {
        return Err(
            "--pc-speaker-frequency and --pc-speaker-duration require --pc-speaker-test".into(),
        );
    }

    Ok(options)
}
