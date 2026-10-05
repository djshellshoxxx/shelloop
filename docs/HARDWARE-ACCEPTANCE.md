# Beta hardware acceptance

Status: NOT RUN. Complete one copy of the results table on Windows and one on Linux. Automated archive checks do not establish audio quality or hardware reliability.

## Candidate download

Open the PR's **Release** workflow run and download the artifact for your OS. Extract the Actions artifact wrapper, then extract the inner Shelloop archive. Keep the patterns directory beside the executable. Run commands from the extracted package directory. On Windows use PowerShell and prefix the executable with `.\\shelloop.exe`; on Linux use `./shelloop`.

Verify the inner archive before extracting it:

- Windows: `Get-FileHash .\\shelloop-v0.01-beta-windows-x86_64.zip -Algorithm SHA256`; compare with the accompanying .sha256 file.
- Linux: `sha256sum -c shelloop-v0.01-beta-linux-x86_64.tar.gz.sha256`.

## Test procedure

1. Run `shelloop --help` and `shelloop --list-devices`. Record output and confirm your audio output and MIDI input are listed.
2. Run `shelloop --no-midi`. Play Z-M and Q-U chords; release all keys, change octaves using brackets, press !, then Esc. Confirm the terminal behaves normally afterward. Record whether the timed keyboard fallback was reported.
3. Run `shelloop --no-midi --pattern patterns/example-bassline.json --bpm 138`. Space starts/stops playback; Backspace restarts. Hold keyboard notes while pausing/restarting the pattern. Confirm the held live notes continue.
4. Run with `--audio-device "EXACT OUTPUT NAME" --midi-name "EXACT INPUT NAME" --pattern patterns/example-bassline.json`. Test MIDI chords, sustain pedal, note releases, and concurrent sequencer playback for at least 10 minutes. Record audible clicks/dropouts and perceived latency; do not report measured latency or xrun counts without measurement tools.
5. Unplug the selected MIDI device while notes are held, then reconnect. Confirm notes stop and input returns. Test the keyboard afterward. Record reconnect duration and terminal messages.
6. Quit and run with `--audio-device "shelloop-nonexistent-device"`. Confirm a readable error and nonzero exit (`$LASTEXITCODE` in PowerShell; `echo $?` on Linux). Repeat with an invalid pattern path. Confirm terminal state remains usable.
7. On a suitable test host, disable or disconnect the active output and observe whether the runtime reports an error and exits cleanly. Also test startup without an available output. Mark unavailable scenarios NOT RUN.

## Results template

| Field | Observation |
|---|---|
| Candidate commit and Actions run URL | |
| Archive SHA-256 | |
| OS/version and terminal | |
| Audio interface, driver, sample rate | |
| MIDI controller and connection | |
| Device listing/default/named output | NOT RUN |
| Keyboard release, octave, panic, quit | NOT RUN |
| Sequencer timing, pause, restart | NOT RUN |
| Concurrent keyboard/MIDI and pattern | NOT RUN |
| Sustain and MIDI unplug/reconnect | NOT RUN |
| Ten-minute playback, latency/dropouts | NOT RUN |
| Missing pattern/invalid device errors | NOT RUN |
| Audio disconnect/no-output behavior | NOT RUN |
| Terminal restored after errors | NOT RUN |

Use PASS, FAIL or NOT RUN with concrete observations. Attach errors and reproduction steps for failures. Any unexecuted gate remains open. If a fix changes runtime code, repeat affected checks against its candidate archive before tagging.
