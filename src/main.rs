use shelloop::parse_startup_options;
use std::process::ExitCode;

const USAGE: &str = "Usage: shelloop [OPTIONS]\n\n\
Terminal-only real-time step sequencer and playable synthesizer.\n\n\
Options:\n\
  --audio-device <NAME>  Select an audio output device by exact name\n\
  --midi-name <NAME>     Select a MIDI input port by exact name\n\
  --midi-index <INDEX>   Select a MIDI input port by zero-based index\n\
  --polyphony <VOICES>   Set synth polyphony from 1 to 256 (default: 16)\n\
  --list-devices         List available audio and MIDI devices\n\
  --no-midi              Disable MIDI input\n\
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

    println!("shelloop: startup options accepted; real-time session wiring is in progress");
    ExitCode::SUCCESS
}
