use shelloop::parse_startup_options;
use std::process::ExitCode;

const USAGE: &str = "Usage: shelloop [OPTIONS]\n\n\
Terminal-only real-time step sequencer and playable synthesizer.\n\n\
Options:\n\
  --audio-device <NAME>  Select an audio output device by exact name\n\
  --midi-name <NAME>     Select a MIDI input port by exact name\n\
  --midi-index <INDEX>   Select a MIDI input port by zero-based index\n\
  --polyphony <VOICES>   Set synth polyphony from 1 to 256 (default: 16)\n\
  --pattern <FILE>       Load a validated JSON pattern for live playback\n\
  --record <FILE>        Record the mono master output to a 32-bit float WAV\n\
  --bpm <TEMPO>          Set sequencer tempo from 20 to 400 (default: 120)\n\
  --steps-per-beat <N>   Set sequencer grid density from 1 to 64 (default: 4)\n\
  --list-devices         List available audio and MIDI devices\n\
  --no-midi              Disable MIDI input\n\
  -V, --version          Print version information\n\
  -h, --help             Print this help text\n";

fn main() -> ExitCode {
    let options = match parse_startup_options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("error: {error}\n\nRun `shelloop --help` for usage.");
            return ExitCode::from(2);
        }
    };

    if options.show_help {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }

    if options.show_version {
        println!("shelloop {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    #[cfg(all(feature = "realtime-audio", feature = "terminal-ui"))]
    {
        match shelloop::run_realtime_session(options) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::from(1)
            }
        }
    }

    #[cfg(not(all(feature = "realtime-audio", feature = "terminal-ui")))]
    {
        let _ = options;
        eprintln!(
            "error: real-time runtime support is not compiled in; rebuild with \
             `--features realtime-audio,terminal-ui` (and `midi` for MIDI input)"
        );
        ExitCode::from(2)
    }
}
