use std::process::Command;

fn shelloop_command() -> Command {
    Command::new(env!("CARGO_BIN_EXE_shelloop"))
}

#[test]
fn help_prints_usage_and_exits_successfully() {
    let output = shelloop_command()
        .arg("--help")
        .output()
        .expect("shelloop binary should launch");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Usage: shelloop"));
    assert!(stdout.contains("--audio-device"));
    assert!(stdout.contains("--midi-name"));
    assert!(stdout.contains("--polyphony"));
}

#[test]
fn invalid_option_reports_error_and_exits_nonzero() {
    let output = shelloop_command()
        .arg("--definitely-not-a-real-option")
        .output()
        .expect("shelloop binary should launch");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("unknown option"));
    assert!(stderr.contains("--help"));
}
