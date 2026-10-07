# Spec 08 — Full-screen Terminal UI

## Objective

Build a ratatui-based full-screen performance and editing interface over the existing engine/control contracts without coupling rendering logic to audio processing.

## User outcome

Shelloop opens as a usable terminal workstation showing transport, tracks, step patterns, meters, device/MIDI state and contextual editors. A user can perform most common operations without memorizing CLI flags or editing JSON.

## Architectural rule

The TUI is a client of the control model. It never reaches into `RealtimeSynth`, effect internals or callback-owned structures. Engine telemetry is published as bounded/coalesced snapshots suitable for UI refresh.

## Main screen layout

Default layout:

```text
┌ Transport / tempo / scene / recording / CPU-status ┐
├ Track list ───────────┬ Pattern grid ───────────────┤
│ 01 DRUMS M S  ▂▅     │ X...X...X...X...            │
│ 02 BASS  M S  ▃▆     │ X..x...X..x.....            │
│ 03 LEAD  M S  ▂▄     │ ....X.......X...            │
├ Inspector ────────────┴─────────────────────────────┤
│ step / synth / sample / FX / MIDI mapping details   │
├ status / command line / errors                       ┤
└──────────────────────────────────────────────────────┘
```

Layout must degrade gracefully for smaller terminals. Define a minimum supported interactive size; below it, display a compact warning/status layout rather than panicking.

## Modes

- Performance mode: transport, scene launching, mute/solo, keyboard playing.
- Pattern edit mode: step navigation and editing.
- Inspector mode: track/synth/sample/effect parameter editing.
- Command mode: textual command entry.
- MIDI learn mode: clearly visible modal/status state.
- Help overlay.

Modes are explicit; key behavior must not depend on hidden focus ambiguity.

## Input routing

Create a pure/testable input router:

```rust
fn route_input(mode: UiMode, event: UiInput) -> Vec<UiAction>;
```

`UiAction` is translated by a controller into existing typed engine/editor commands. The router itself performs no audio work.

Computer-keyboard musical performance must remain possible. When text command entry is focused, note keys type text instead of triggering notes. Escape exits the focused mode before it exits the application; application quit should require the established quit action or a deliberate second-level command.

## Pattern grid

Required grid behavior:

- Selected track row/pattern.
- 16-step viewport with horizontal scrolling for longer patterns.
- Distinct glyph/state for active step, probability-reduced step, ratcheted step and current playhead.
- Cursor selection.
- Toggle step.
- Inspector values for note, velocity, gate, probability, ratchets, microtiming.
- Queued-versus-active revision indicator.

Do not attempt to encode every parameter as color alone. Glyphs/text preserve usability in monochrome terminals.

## Track pane

Show track number/name/type, mute/solo, gain indicator, pan, activity/meter, and currently active pattern. Track selection drives inspector context.

## Transport/status bar

Show at least BPM, play/pause, absolute bar/beat, active/queued scene, recording state, MIDI connection state and audio device. Do not display made-up CPU/xrun statistics; only show measurements the runtime actually collects.

## Meters

Audio thread may publish peak/RMS telemetry through atomics or a bounded latest-value channel. UI refresh rate target 20–30 Hz. Meter updates are expendable; dropping intermediate telemetry is preferred to backpressure.

## Render loop

Terminal rendering occurs on the UI/control thread at a bounded refresh cadence. Input should wake/redraw as needed. The UI must restore raw mode, alternate screen, cursor and mouse capture on all normal error/quit paths through RAII guards.

## Mouse

Existing `--mouse-xy` mode and TUI mouse interactions need explicit arbitration. In full-screen TUI, mouse XY performance should activate only within a dedicated XY pad widget or a deliberate modifier/mode, not across the entire interface.

## Command palette

Provide `:` command mode or equivalent using the typed command parser. Command history is bounded. Initial autocomplete may operate on static command/track/scene names.

## Accessibility/terminal compatibility

- Unicode enhanced mode with ASCII fallback.
- No correctness dependency on color.
- Configurable refresh rate if low-bandwidth SSH use is later supported.
- Detect keyboard enhancement support using current logic.

## Tests

Write failing tests first for:

- Pure key routing per mode.
- Escape hierarchy.
- Grid viewport/cursor behavior at 1, 16, 17 and 256 steps.
- Small-terminal layout never panics.
- Playhead calculation from engine telemetry.
- Queued/active edit indicators.
- Track mute/solo actions route to correct stable TrackId.
- Mouse clicks map only to widget bounds.
- Meter telemetry dropping does not block engine.
- Terminal guard restores state on injected errors.
- Snapshot rendering tests use narrowly scoped stable textual sections, not huge fragile screen snapshots.

## Completion criteria

The full-screen TUI can load and operate an existing project, edit patterns, control tracks/scenes/synth/FX parameters, show truthful runtime status and exit cleanly. Headless/basic CLI operation remains supported for scripting and troubleshooting.