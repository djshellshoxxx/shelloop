//! Contracts for the full-screen terminal UI building blocks (spec 08).
//!
//! The pure parts (routing, viewport, layout, glyphs, history, terminal
//! restore logic) are tested without any terminal. Rendering tests run only
//! with the `terminal-ui` feature and assert small, stable text fragments.

use shelloop::multitrack::{TrackId, TrackKind};
use shelloop::tui::*;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn key(k: UiKey) -> UiInput {
    UiInput::key(k)
}

fn ch(c: char) -> UiInput {
    UiInput::char(c)
}

fn release(c: char) -> UiInput {
    UiInput::Key {
        key: UiKey::Char(c),
        ctrl: false,
        alt: false,
        shift: false,
        release: true,
    }
}

fn shifted(k: UiKey) -> UiInput {
    UiInput::Key {
        key: k,
        ctrl: false,
        alt: false,
        shift: true,
        release: false,
    }
}

fn ctrl(c: char) -> UiInput {
    UiInput::Key {
        key: UiKey::Char(c),
        ctrl: true,
        alt: false,
        shift: false,
        release: false,
    }
}

fn mouse(column: u16, row: u16, kind: UiMouseKind) -> UiInput {
    UiInput::Mouse { column, row, kind }
}

fn route(mode: UiMode, input: UiInput) -> Vec<UiAction> {
    route_input(mode, &input, None, None)
}

fn track(id: u16, name: &str, muted: bool, soloed: bool) -> TrackView {
    TrackView {
        id: TrackId(id),
        name: name.to_string(),
        kind: TrackKind::Synth,
        muted,
        soloed,
        audible: !muted,
        gain: 0.8,
        pan: 0.0,
        peak: 0.5,
        pattern_label: "main".to_string(),
        revision_state: RevisionState::Active(1),
    }
}

fn active_step() -> StepView {
    StepView {
        note: 60,
        velocity: 0.8,
        gate: 0.5,
        probability: 1.0,
        ratchets: 1,
        microtiming: 0,
    }
}

fn snapshot(len: usize) -> TuiSnapshot {
    let mut steps = vec![None; len];
    for (index, step) in steps.iter_mut().enumerate() {
        if index % 4 == 0 {
            *step = Some(active_step());
        }
    }
    TuiSnapshot {
        transport: TransportView {
            playing: true,
            bpm: 128.0,
            bar: 3,
            beat: 2,
            step_in_beat: 1,
            active_scene: Some("verse".to_string()),
            queued_scene: Some(("chorus".to_string(), 5)),
            chain: None,
            recording: None,
            black_box: None,
            midi: Some("Launchkey".to_string()),
            audio_device: "default".to_string(),
            sample_rate: 48_000,
            resample: None,
        },
        // Stable IDs deliberately differ from list positions.
        tracks: vec![
            track(7, "DRUMS", false, false),
            track(3, "BASS", true, false),
            track(12, "LEAD", false, true),
        ],
        selected_track: 0,
        grid: GridView {
            steps,
            lock_counts: vec![0; len],
            viewport: GridViewport::new(len, 16),
            playhead: Some(0),
        },
        inspector: InspectorView {
            title: "Step 1".to_string(),
            rows: vec![
                ("note".to_string(), "C4".to_string()),
                ("velocity".to_string(), "0.80".to_string()),
            ],
            selected: 0,
        },
        status: vec![StatusLine {
            text: "ready".to_string(),
            error: false,
        }],
        command: None,
        mode: UiMode::Performance,
        learn: None,
        ascii: false,
    }
}

// ---------------------------------------------------------------------------
// Key routing per mode
// ---------------------------------------------------------------------------

#[test]
fn performance_mode_plays_the_musical_keyboard() {
    assert_eq!(
        route(UiMode::Performance, ch('z')),
        vec![UiAction::NoteOn {
            key: 'z',
            offset: 0
        }]
    );
    assert_eq!(
        route(UiMode::Performance, ch('m')),
        vec![UiAction::NoteOn {
            key: 'm',
            offset: 11
        }]
    );
    assert_eq!(
        route(UiMode::Performance, ch('q')),
        vec![UiAction::NoteOn {
            key: 'q',
            offset: 12
        }]
    );
    assert_eq!(
        route(UiMode::Performance, ch('u')),
        vec![UiAction::NoteOn {
            key: 'u',
            offset: 23
        }]
    );
    assert_eq!(
        route(UiMode::Performance, release('z')),
        vec![UiAction::NoteOff {
            key: 'z',
            offset: 0
        }]
    );
    // Releases of non-note keys are ignored.
    assert!(route(UiMode::Performance, release('[')).is_empty());
}

#[test]
fn performance_mode_transport_and_scene_keys() {
    let p = UiMode::Performance;
    assert_eq!(route(p, ch('[')), vec![UiAction::OctaveDown]);
    assert_eq!(route(p, ch(']')), vec![UiAction::OctaveUp]);
    assert_eq!(route(p, ch('!')), vec![UiAction::Panic]);
    assert_eq!(route(p, ch(' ')), vec![UiAction::TogglePlay]);
    assert_eq!(route(p, key(UiKey::Backspace)), vec![UiAction::Restart]);
    assert_eq!(route(p, ch('<')), vec![UiAction::ScenePrev]);
    assert_eq!(route(p, ch('>')), vec![UiAction::SceneNext]);
    assert_eq!(
        route(p, key(UiKey::F(2))),
        vec![UiAction::LaunchSceneSlot(0)]
    );
    assert_eq!(
        route(p, key(UiKey::F(7))),
        vec![UiAction::LaunchSceneSlot(5)]
    );
    assert_eq!(route(p, key(UiKey::F(8))), vec![UiAction::BlackBoxSave]);
    assert_eq!(
        route(p, key(UiKey::F(9))),
        vec![UiAction::ToggleMuteSelected]
    );
    assert_eq!(
        route(p, key(UiKey::F(10))),
        vec![UiAction::ToggleSoloSelected]
    );
    assert_eq!(route(p, key(UiKey::Up)), vec![UiAction::SelectTrack(-1)]);
    assert_eq!(route(p, key(UiKey::Down)), vec![UiAction::SelectTrack(1)]);
    assert_eq!(
        route(p, key(UiKey::F(1))),
        vec![UiAction::SetMode(UiMode::Help)]
    );
    assert_eq!(route(p, ch('?')), vec![UiAction::SetMode(UiMode::Help)]);
    assert_eq!(route(p, ch(':')), vec![UiAction::SetMode(UiMode::Command)]);
    assert_eq!(
        route(p, key(UiKey::Tab)),
        vec![UiAction::SetMode(UiMode::PatternEdit)]
    );
    assert_eq!(route(p, ch('~')), vec![UiAction::Quit]);
    assert_eq!(route(p, ctrl('q')), vec![UiAction::Quit]);
}

#[test]
fn pattern_edit_mode_keys() {
    let e = UiMode::PatternEdit;
    assert_eq!(route(e, key(UiKey::Left)), vec![UiAction::CursorMove(-1)]);
    assert_eq!(route(e, key(UiKey::Right)), vec![UiAction::CursorMove(1)]);
    assert_eq!(route(e, key(UiKey::Home)), vec![UiAction::CursorHome]);
    assert_eq!(route(e, key(UiKey::End)), vec![UiAction::CursorEnd]);
    assert_eq!(route(e, key(UiKey::Enter)), vec![UiAction::ToggleStep]);
    assert_eq!(route(e, ch(' ')), vec![UiAction::ToggleStep]);
    assert_eq!(
        route(e, key(UiKey::PageDown)),
        vec![UiAction::ScrollGrid(16)]
    );
    assert_eq!(
        route(e, key(UiKey::PageUp)),
        vec![UiAction::ScrollGrid(-16)]
    );
    assert_eq!(route(e, key(UiKey::Up)), vec![UiAction::SelectTrack(-1)]);
    assert_eq!(
        route(e, key(UiKey::Tab)),
        vec![UiAction::SetMode(UiMode::Inspector)]
    );
    let fields = [
        ('n', StepField::Note),
        ('v', StepField::Velocity),
        ('g', StepField::Gate),
        ('p', StepField::Probability),
        ('r', StepField::Ratchets),
        ('t', StepField::Microtiming),
    ];
    for (c, field) in fields {
        assert_eq!(
            route(e, ch(c)),
            vec![UiAction::StepField { field, delta: -1 }]
        );
        assert_eq!(
            route(e, ch(c.to_ascii_uppercase())),
            vec![UiAction::StepField { field, delta: 1 }]
        );
    }
    // Note keys do not play notes while editing.
    assert!(!route(e, ch('z'))
        .iter()
        .any(|a| matches!(a, UiAction::NoteOn { .. })));
}

#[test]
fn inspector_mode_keys() {
    let i = UiMode::Inspector;
    assert_eq!(route(i, key(UiKey::Up)), vec![UiAction::InspectorMove(-1)]);
    assert_eq!(route(i, key(UiKey::Down)), vec![UiAction::InspectorMove(1)]);
    assert_eq!(
        route(i, key(UiKey::Right)),
        vec![UiAction::InspectorAdjust {
            delta: 1,
            coarse: false
        }]
    );
    assert_eq!(
        route(i, shifted(UiKey::Left)),
        vec![UiAction::InspectorAdjust {
            delta: -1,
            coarse: true
        }]
    );
    assert_eq!(
        route(i, ch('l')),
        vec![
            UiAction::LearnSelectedParameter,
            UiAction::SetMode(UiMode::MidiLearn)
        ]
    );
}

#[test]
fn text_entry_does_not_trigger_notes() {
    for c in "zsxdcvgbhnjmq2w3er5t6y7u[]! ~?<>".chars() {
        let actions = route(UiMode::Command, ch(c));
        assert_eq!(actions, vec![UiAction::CommandInput(c)], "char {c:?}");
    }
    // Releases while typing never produce note-offs either.
    assert!(route(UiMode::Command, release('z')).is_empty());
    assert_eq!(
        route(UiMode::Command, key(UiKey::Backspace)),
        vec![UiAction::CommandBackspace]
    );
    assert_eq!(
        route(UiMode::Command, key(UiKey::Enter)),
        vec![
            UiAction::CommandSubmit,
            UiAction::SetMode(UiMode::Performance)
        ]
    );
    assert_eq!(
        route(UiMode::Command, key(UiKey::Up)),
        vec![UiAction::CommandHistory(-1)]
    );
    assert_eq!(
        route(UiMode::Command, key(UiKey::Down)),
        vec![UiAction::CommandHistory(1)]
    );
    assert_eq!(
        route(UiMode::Command, key(UiKey::Tab)),
        vec![UiAction::CommandComplete]
    );
}

#[test]
fn midi_learn_mode_keys() {
    assert_eq!(
        route(UiMode::MidiLearn, key(UiKey::Enter)),
        vec![
            UiAction::ConfirmLearn,
            UiAction::SetMode(UiMode::Performance)
        ]
    );
    assert!(route(UiMode::MidiLearn, ch('z')).is_empty());
}

#[test]
fn help_closes_on_any_key() {
    for input in [ch('z'), key(UiKey::Enter), key(UiKey::F(5)), ch('?')] {
        assert_eq!(
            route(UiMode::Help, input),
            vec![UiAction::SetMode(UiMode::Performance)]
        );
    }
    assert!(route(UiMode::Help, release('z')).is_empty());
}

// ---------------------------------------------------------------------------
// Escape hierarchy
// ---------------------------------------------------------------------------

#[test]
fn escape_leaves_the_focused_mode_before_quitting() {
    let esc = key(UiKey::Esc);
    assert_eq!(
        route(UiMode::PatternEdit, esc.clone()),
        vec![UiAction::SetMode(UiMode::Performance)]
    );
    assert_eq!(
        route(UiMode::Inspector, esc.clone()),
        vec![UiAction::SetMode(UiMode::Performance)]
    );
    assert_eq!(
        route(UiMode::Command, esc.clone()),
        vec![
            UiAction::CommandCancel,
            UiAction::SetMode(UiMode::Performance)
        ]
    );
    assert_eq!(
        route(UiMode::MidiLearn, esc.clone()),
        vec![
            UiAction::CancelLearn,
            UiAction::SetMode(UiMode::Performance)
        ]
    );
    assert_eq!(
        route(UiMode::Help, esc.clone()),
        vec![UiAction::SetMode(UiMode::Performance)]
    );
    // Escape at the top level never quits.
    let top = route(UiMode::Performance, esc);
    assert!(!top.contains(&UiAction::Quit));
    assert_eq!(top, vec![UiAction::ClearStatus]);
}

#[test]
fn every_mode_escapes_to_performance_without_quitting() {
    for mode in UiMode::ALL {
        let actions = route(mode, key(UiKey::Esc));
        assert!(!actions.contains(&UiAction::Quit), "{mode:?}");
        if mode != UiMode::Performance {
            assert!(
                actions.contains(&UiAction::SetMode(UiMode::Performance)),
                "{mode:?}"
            );
        }
    }
}

#[test]
fn resize_is_forwarded_in_every_mode() {
    for mode in UiMode::ALL {
        assert_eq!(
            route(
                mode,
                UiInput::Resize {
                    width: 80,
                    height: 24
                }
            ),
            vec![UiAction::Resize {
                width: 80,
                height: 24
            }]
        );
    }
}

#[test]
fn help_lines_document_the_key_map() {
    let help = HELP_LINES.join("\n");
    for fragment in ["Ctrl+Q", "Tab", "Esc", "F8", "F9", "F10", ":", "?"] {
        assert!(help.contains(fragment), "missing {fragment}");
    }
}

// ---------------------------------------------------------------------------
// Grid viewport
// ---------------------------------------------------------------------------

#[test]
fn viewport_with_one_step() {
    let mut vp = GridViewport::new(1, 16);
    assert_eq!(vp.visible_range(), 0..1);
    vp.move_cursor(5);
    assert_eq!(vp.cursor, 0);
    vp.move_cursor(-5);
    assert_eq!(vp.cursor, 0);
    vp.scroll(16);
    assert_eq!(vp.offset, 0);
    assert_eq!(vp.visible_range(), 0..1);
}

#[test]
fn viewport_with_sixteen_steps_never_scrolls() {
    let mut vp = GridViewport::new(16, 16);
    vp.move_cursor(15);
    assert_eq!(vp.cursor, 15);
    assert_eq!(vp.offset, 0);
    vp.move_cursor(1);
    assert_eq!(vp.cursor, 15);
    vp.scroll(16);
    assert_eq!(vp.offset, 0);
    assert_eq!(vp.visible_range(), 0..16);
}

#[test]
fn viewport_with_seventeen_steps_scrolls_by_one() {
    let mut vp = GridViewport::new(17, 16);
    vp.set_cursor(16);
    assert_eq!(vp.cursor, 16);
    assert_eq!(vp.offset, 1);
    assert_eq!(vp.visible_range(), 1..17);
    vp.set_cursor(0);
    assert_eq!(vp.offset, 0);
    vp.scroll(16);
    assert_eq!(vp.offset, 1);
    assert!(vp.visible_range().contains(&vp.cursor));
}

#[test]
fn viewport_with_256_steps_pages_and_keeps_cursor_visible() {
    let mut vp = GridViewport::new(256, 16);
    vp.scroll(16);
    assert_eq!(vp.visible_range(), 16..32);
    assert!(vp.visible_range().contains(&vp.cursor));
    vp.set_cursor(255);
    assert_eq!(vp.visible_range(), 240..256);
    vp.move_cursor(-16);
    assert_eq!(vp.cursor, 239);
    assert!(vp.visible_range().contains(&239));
    vp.set_cursor(10_000);
    assert_eq!(vp.cursor, 255);
    vp.scroll(-1_000);
    assert_eq!(vp.offset, 0);
    assert!(vp.visible_range().contains(&vp.cursor));
    for i in 0..256 {
        vp.set_cursor(i);
        assert!(vp.visible_range().contains(&i));
        assert_eq!(vp.visible_range().len(), 16);
    }
}

#[test]
fn viewport_with_degenerate_widths_and_lengths_does_not_panic() {
    for len in [0, 1, 16, 17, 256] {
        for width in [0, 1, 16, 300] {
            let mut vp = GridViewport::new(len, width);
            for delta in [-1000, -17, -1, 0, 1, 16, 17, 1000] {
                vp.move_cursor(delta);
                vp.scroll(delta);
                let range = vp.visible_range();
                assert!(range.end <= len);
                if len > 0 {
                    assert!(vp.cursor < len);
                    assert!(range.contains(&vp.cursor), "len {len} width {width}");
                }
            }
            vp.set_cursor(usize::MAX);
            vp.set_len(len / 2);
            assert!(vp.visible_range().end <= len / 2);
        }
    }
}

// ---------------------------------------------------------------------------
// Playhead
// ---------------------------------------------------------------------------

#[test]
fn playhead_from_engine_position() {
    assert_eq!(playhead_step(0, 6000.0, 16), Some(0));
    assert_eq!(playhead_step(5999, 6000.0, 16), Some(0));
    assert_eq!(playhead_step(6000, 6000.0, 16), Some(1));
    assert_eq!(playhead_step(6000 * 17, 6000.0, 16), Some(1));
    assert_eq!(playhead_step(6000 * 255, 6000.0, 256), Some(255));
    assert_eq!(playhead_step(10, 6000.0, 0), None);
    assert_eq!(playhead_step(10, 0.0, 16), None);
    assert_eq!(playhead_step(10, -1.0, 16), None);
    assert_eq!(playhead_step(10, f64::NAN, 16), None);
    assert_eq!(playhead_step(10, f64::INFINITY, 16), None);
    assert_eq!(playhead_step(u64::MAX, 1.0, 7).map(|s| s < 7), Some(true));
}

// ---------------------------------------------------------------------------
// Glyphs and meters
// ---------------------------------------------------------------------------

#[test]
fn step_glyphs_are_distinct_without_color() {
    for ascii in [false, true] {
        let active = active_step();
        let reduced = StepView {
            probability: 0.5,
            ..active
        };
        let ratcheted = StepView {
            ratchets: 3,
            ..active
        };
        let glyphs = [
            step_glyph(None, 0, false, false, ascii),
            step_glyph(Some(&active), 0, false, false, ascii),
            step_glyph(Some(&reduced), 0, false, false, ascii),
            step_glyph(Some(&ratcheted), 0, false, false, ascii),
            step_glyph(None, 2, false, false, ascii),
            step_glyph(None, 0, true, false, ascii),
            step_glyph(Some(&active), 0, true, false, ascii),
        ];
        for (i, a) in glyphs.iter().enumerate() {
            for b in &glyphs[i + 1..] {
                assert_ne!(a, b, "ascii={ascii} glyphs {glyphs:?}");
            }
        }
        if ascii {
            assert!(glyphs.iter().all(char::is_ascii));
        }
    }
}

#[test]
fn meter_bar_has_fixed_width_and_marks_clipping() {
    for ascii in [false, true] {
        for peak in [-1.0, 0.0, 0.3, 1.0, 2.0, f32::NAN] {
            for width in [0, 1, 4, 10] {
                assert_eq!(meter_bar(peak, width, ascii).chars().count(), width);
            }
        }
    }
    assert_eq!(meter_bar(0.0, 4, true), "....");
    assert_eq!(meter_bar(1.0, 4, true), "####");
    assert_eq!(meter_bar(0.5, 4, true), "##..");
    assert!(meter_bar(1.5, 4, true).ends_with('!'));
    assert!(meter_bar(1.5, 4, false).ends_with('!'));
}

#[test]
fn revision_indicator_text() {
    assert_eq!(RevisionState::Active(4).label(), "rev 4");
    assert_eq!(
        RevisionState::Queued {
            active: 4,
            queued: 5
        }
        .label(),
        "rev 4 (queued 5)"
    );
}

// ---------------------------------------------------------------------------
// Layout and mouse
// ---------------------------------------------------------------------------

fn sample_sizes() -> Vec<(u16, u16)> {
    let mut sizes = Vec::new();
    for w in (0..=200).step_by(7).chain([1, 59, 60, 61, 199, 200]) {
        for h in (0..=60).step_by(5).chain([1, 17, 18, 19, 59, 60]) {
            sizes.push((w, h));
        }
    }
    sizes
}

fn inside(inner: Rect, outer: Rect) -> bool {
    inner.width == 0
        || inner.height == 0
        || (inner.x >= outer.x
            && inner.y >= outer.y
            && inner.x as u32 + inner.width as u32 <= outer.x as u32 + outer.width as u32
            && inner.y as u32 + inner.height as u32 <= outer.y as u32 + outer.height as u32)
}

#[test]
fn layout_never_panics_and_stays_on_screen() {
    for (w, h) in sample_sizes() {
        let layout = TuiLayout::compute(w, h);
        let screen = Rect {
            x: 0,
            y: 0,
            width: w,
            height: h,
        };
        assert_eq!(layout.compact, w < MIN_TUI_COLUMNS || h < MIN_TUI_ROWS);
        for rect in [
            layout.transport,
            layout.tracks,
            layout.grid,
            layout.inspector,
            layout.status,
        ] {
            assert!(inside(rect, screen), "{w}x{h}: {rect:?}");
        }
        if let Some(pad) = layout.xy_pad {
            assert!(inside(pad, layout.inspector), "{w}x{h}: {pad:?}");
        }
        // Hit testing every cell never panics.
        let snap = snapshot(32);
        for column in (0..w.saturating_add(2)).step_by(3) {
            for row in 0..h.saturating_add(2) {
                let _ = layout.hit_test(column, row, Some(&snap));
            }
        }
    }
}

#[test]
fn full_layout_has_an_xy_pad_at_common_sizes() {
    let layout = TuiLayout::compute(100, 30);
    assert!(!layout.compact);
    assert!(layout.xy_pad.is_some());
    assert!(layout.grid.width > 16);
}

#[test]
fn mouse_mute_and_solo_route_to_the_stable_track_id() {
    let layout = TuiLayout::compute(100, 30);
    let snap = snapshot(16);
    let row_of = |index: u16| layout.tracks.y + 1 + index;
    let col = |offset: u16| layout.tracks.x + 1 + offset;

    // Row 1 is "BASS", stable id 3.
    let actions = route_input(
        UiMode::Performance,
        &mouse(col(TRACK_MUTE_COLUMN), row_of(1), UiMouseKind::Down),
        Some(&layout),
        Some(&snap),
    );
    assert_eq!(actions, vec![UiAction::ToggleMute(TrackId(3))]);

    // Row 2 is "LEAD", stable id 12.
    let actions = route_input(
        UiMode::PatternEdit,
        &mouse(col(TRACK_SOLO_COLUMN), row_of(2), UiMouseKind::Down),
        Some(&layout),
        Some(&snap),
    );
    assert_eq!(actions, vec![UiAction::ToggleSolo(TrackId(12))]);

    // Clicking the name selects by list index.
    let actions = route_input(
        UiMode::Performance,
        &mouse(col(5), row_of(2), UiMouseKind::Down),
        Some(&layout),
        Some(&snap),
    );
    assert_eq!(actions, vec![UiAction::SelectTrackIndex(2)]);

    assert_eq!(
        layout.hit_test(col(TRACK_MUTE_COLUMN), row_of(0), Some(&snap)),
        Some(UiTarget::TrackRow {
            index: 0,
            track: TrackId(7),
            column_kind: TrackColumn::Mute
        })
    );
    // Below the last track: nothing.
    assert_eq!(
        layout.hit_test(col(TRACK_MUTE_COLUMN), row_of(3), Some(&snap)),
        None
    );
}

#[test]
fn grid_clicks_move_cursor_and_toggle_on_second_click() {
    let layout = TuiLayout::compute(100, 30);
    let mut snap = snapshot(32);
    snap.grid.viewport.scroll(16);
    let offset = snap.grid.viewport.offset;
    assert_eq!(offset, 16);
    let row = layout.grid.y + 1 + GRID_STEP_ROW;
    let column = layout.grid.x + 1 + 3;
    assert_eq!(
        layout.hit_test(column, row, Some(&snap)),
        Some(UiTarget::GridStep(19))
    );
    let first = route_input(
        UiMode::PatternEdit,
        &mouse(column, row, UiMouseKind::Down),
        Some(&layout),
        Some(&snap),
    );
    assert_eq!(first, vec![UiAction::CursorTo(19)]);

    snap.grid.viewport.set_cursor(19);
    let second = route_input(
        UiMode::PatternEdit,
        &mouse(column, row, UiMouseKind::Down),
        Some(&layout),
        Some(&snap),
    );
    assert_eq!(second, vec![UiAction::CursorTo(19), UiAction::ToggleStep]);

    // Past the end of a short pattern there is no step.
    let short = snapshot(4);
    assert_eq!(
        layout.hit_test(layout.grid.x + 1 + 6, row, Some(&short)),
        None
    );
}

#[test]
fn mouse_outside_widgets_produces_no_actions() {
    let layout = TuiLayout::compute(100, 30);
    let snap = snapshot(16);
    let outside = [
        (layout.transport.x + 3, layout.transport.y + 1),
        (layout.status.x + 3, layout.status.y),
        (layout.inspector.x + 2, layout.inspector.y + 1),
        (layout.tracks.x, layout.tracks.y),
        (99, 29),
        (500, 500),
    ];
    for (column, row) in outside {
        for kind in [UiMouseKind::Down, UiMouseKind::Drag, UiMouseKind::Up] {
            let actions = route_input(
                UiMode::Performance,
                &mouse(column, row, kind),
                Some(&layout),
                Some(&snap),
            );
            assert!(actions.is_empty(), "({column},{row}) {kind:?}: {actions:?}");
        }
    }
    // Without a layout no mouse event does anything.
    assert!(route(UiMode::Performance, mouse(10, 10, UiMouseKind::Down)).is_empty());
    // A compact layout has no interactive widgets.
    let compact = TuiLayout::compute(30, 10);
    for column in 0..30 {
        for row in 0..10 {
            assert!(route_input(
                UiMode::Performance,
                &mouse(column, row, UiMouseKind::Down),
                Some(&compact),
                Some(&snap),
            )
            .is_empty());
        }
    }
}

#[test]
fn xy_pad_normalizes_with_y_up() {
    let layout = TuiLayout::compute(100, 30);
    let pad = layout.xy_pad.expect("xy pad");
    let snap = snapshot(16);
    let xy = |column: u16, row: u16| {
        route_input(
            UiMode::Performance,
            &mouse(column, row, UiMouseKind::Down),
            Some(&layout),
            Some(&snap),
        )
    };
    let right = pad.x + pad.width - 1;
    let bottom = pad.y + pad.height - 1;
    assert_eq!(xy(pad.x, pad.y), vec![UiAction::XyPad { x: 0.0, y: 1.0 }]);
    assert_eq!(xy(right, bottom), vec![UiAction::XyPad { x: 1.0, y: 0.0 }]);
    let drag = route_input(
        UiMode::Inspector,
        &mouse(right, pad.y, UiMouseKind::Drag),
        Some(&layout),
        Some(&snap),
    );
    assert_eq!(drag, vec![UiAction::XyPad { x: 1.0, y: 1.0 }]);
    let up = route_input(
        UiMode::Inspector,
        &mouse(right, pad.y, UiMouseKind::Up),
        Some(&layout),
        Some(&snap),
    );
    assert_eq!(up, vec![UiAction::XyPadRelease]);
    for column in pad.x..pad.x + pad.width {
        for row in pad.y..pad.y + pad.height {
            match layout.hit_test(column, row, Some(&snap)) {
                Some(UiTarget::XyPad { x, y }) => {
                    assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y));
                }
                other => panic!("expected pad at ({column},{row}), got {other:?}"),
            }
        }
    }
    // One cell outside the pad interior is not the pad.
    assert!(!matches!(
        layout.hit_test(pad.x.saturating_sub(1), pad.y, Some(&snap)),
        Some(UiTarget::XyPad { .. })
    ));
}

#[test]
fn modal_modes_ignore_mouse() {
    let layout = TuiLayout::compute(100, 30);
    let snap = snapshot(16);
    let pad = layout.xy_pad.unwrap();
    for mode in [UiMode::Command, UiMode::MidiLearn, UiMode::Help] {
        assert!(route_input(
            mode,
            &mouse(pad.x, pad.y, UiMouseKind::Down),
            Some(&layout),
            Some(&snap)
        )
        .is_empty());
    }
}

// ---------------------------------------------------------------------------
// Command history and completion
// ---------------------------------------------------------------------------

#[test]
fn command_history_is_bounded_and_navigable() {
    let mut history = CommandHistory::new();
    for i in 0..100 {
        history.push(&format!("cmd {i}"));
    }
    assert_eq!(history.len(), COMMAND_HISTORY_CAPACITY);
    assert_eq!(COMMAND_HISTORY_CAPACITY, 64);
    assert_eq!(history.prev(), Some("cmd 99"));
    assert_eq!(history.prev(), Some("cmd 98"));
    assert_eq!(history.next(), Some("cmd 99"));
    assert_eq!(history.next(), None);
    for _ in 0..200 {
        history.prev();
    }
    assert_eq!(history.prev(), Some("cmd 36"));

    let mut small = CommandHistory::new();
    small.push("  ");
    assert!(small.is_empty());
    small.push("swing 0.2");
    small.push("swing 0.2");
    assert_eq!(small.len(), 1);
    assert_eq!(small.next(), None);
}

#[test]
fn completion_is_longest_unique_prefix() {
    assert_eq!(
        complete_command("sy", COMMAND_WORDS),
        Some("synth".to_string())
    );
    assert_eq!(
        complete_command("bl", COMMAND_WORDS),
        Some("blackbox".to_string())
    );
    assert_eq!(complete_command("unl", COMMAND_WORDS), None);
    assert_eq!(complete_command("un", COMMAND_WORDS), None);
    assert_eq!(
        complete_command("map", COMMAND_WORDS),
        Some("mapping".to_string())
    );
    assert_eq!(complete_command("s", COMMAND_WORDS), None);
    assert_eq!(complete_command("lock", COMMAND_WORDS), None);
    assert_eq!(complete_command("", COMMAND_WORDS), None);
    assert_eq!(complete_command("zzz", COMMAND_WORDS), None);
    for word in [
        "track",
        "step",
        "length",
        "swing",
        "rotate",
        "undo",
        "redo",
        "synth",
        "scene",
        "chain",
        "lock",
        "unlock",
        "locks",
        "learn",
        "unlearn",
        "mappings",
        "mapping",
        "fx",
        "send",
        "master",
        "resample",
        "blackbox",
        "variation",
        "save",
        "help",
        "quit",
    ] {
        assert!(COMMAND_WORDS.contains(&word), "{word}");
    }
}

// ---------------------------------------------------------------------------
// Terminal guard restore logic
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct FakeTerminal {
    log: Vec<&'static str>,
    fail_on: Option<&'static str>,
}

impl FakeTerminal {
    fn step(&mut self, name: &'static str) -> Result<(), String> {
        self.log.push(name);
        if self.fail_on == Some(name) {
            Err(format!("injected failure in {name}"))
        } else {
            Ok(())
        }
    }
}

impl TerminalOps for FakeTerminal {
    fn enable_raw_mode(&mut self) -> Result<(), String> {
        self.step("enable_raw")
    }
    fn disable_raw_mode(&mut self) -> Result<(), String> {
        self.step("disable_raw")
    }
    fn enter_alternate_screen(&mut self) -> Result<(), String> {
        self.step("enter_alt")
    }
    fn leave_alternate_screen(&mut self) -> Result<(), String> {
        self.step("leave_alt")
    }
    fn hide_cursor(&mut self) -> Result<(), String> {
        self.step("hide_cursor")
    }
    fn show_cursor(&mut self) -> Result<(), String> {
        self.step("show_cursor")
    }
    fn enable_mouse_capture(&mut self) -> Result<(), String> {
        self.step("enable_mouse")
    }
    fn disable_mouse_capture(&mut self) -> Result<(), String> {
        self.step("disable_mouse")
    }
}

#[test]
fn guard_restores_everything_on_drop() {
    let session = TerminalSession::enter(FakeTerminal::default(), true).expect("enter");
    assert_eq!(
        session.state(),
        TerminalState {
            raw: true,
            alternate_screen: true,
            cursor_hidden: true,
            mouse_capture: true
        }
    );
    let ops = session.restore_and_take().expect("ops");
    assert_eq!(
        ops.log,
        vec![
            "enable_raw",
            "enter_alt",
            "hide_cursor",
            "enable_mouse",
            "disable_mouse",
            "show_cursor",
            "leave_alt",
            "disable_raw",
        ]
    );
}

#[test]
fn guard_restores_applied_steps_when_entering_fails() {
    let ops = FakeTerminal {
        fail_on: Some("enable_mouse"),
        ..FakeTerminal::default()
    };
    let (error, ops) = match TerminalSession::enter(ops, true) {
        Ok(_) => panic!("expected injected failure"),
        Err(failure) => failure,
    };
    assert!(error.contains("injected"));
    assert_eq!(
        ops.log,
        vec![
            "enable_raw",
            "enter_alt",
            "hide_cursor",
            "enable_mouse",
            "show_cursor",
            "leave_alt",
            "disable_raw",
        ]
    );
}

#[test]
fn guard_restores_when_the_ui_body_returns_an_error() {
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Shared(Rc<RefCell<Vec<&'static str>>>);
    impl Shared {
        fn step(&mut self, name: &'static str) -> Result<(), String> {
            self.0.borrow_mut().push(name);
            Ok(())
        }
    }
    impl TerminalOps for Shared {
        fn enable_raw_mode(&mut self) -> Result<(), String> {
            self.step("enable_raw")
        }
        fn disable_raw_mode(&mut self) -> Result<(), String> {
            self.step("disable_raw")
        }
        fn enter_alternate_screen(&mut self) -> Result<(), String> {
            self.step("enter_alt")
        }
        fn leave_alternate_screen(&mut self) -> Result<(), String> {
            self.step("leave_alt")
        }
        fn hide_cursor(&mut self) -> Result<(), String> {
            self.step("hide_cursor")
        }
        fn show_cursor(&mut self) -> Result<(), String> {
            self.step("show_cursor")
        }
        fn enable_mouse_capture(&mut self) -> Result<(), String> {
            self.step("enable_mouse")
        }
        fn disable_mouse_capture(&mut self) -> Result<(), String> {
            self.step("disable_mouse")
        }
    }

    let log = Rc::new(RefCell::new(Vec::new()));
    let run = || -> Result<(), String> {
        let _guard = TerminalSession::enter(Shared(log.clone()), false).map_err(|(e, _)| e)?;
        Err("injected render error".to_string())
    };
    assert!(run().is_err());
    let log = log.borrow();
    assert_eq!(log.first(), Some(&"enable_raw"));
    assert!(!log.contains(&"enable_mouse"));
    assert!(!log.contains(&"disable_mouse"));
    assert_eq!(
        &log[log.len() - 3..],
        &["show_cursor", "leave_alt", "disable_raw"]
    );
}

#[test]
fn restore_attempts_every_step_even_if_one_fails() {
    let mut ops = FakeTerminal {
        fail_on: Some("show_cursor"),
        ..FakeTerminal::default()
    };
    let result = restore_terminal(
        &mut ops,
        TerminalState {
            raw: true,
            alternate_screen: true,
            cursor_hidden: true,
            mouse_capture: true,
        },
    );
    assert!(result.is_err());
    assert_eq!(
        ops.log,
        vec!["disable_mouse", "show_cursor", "leave_alt", "disable_raw"]
    );
}

// ---------------------------------------------------------------------------
// Rendering (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "terminal-ui")]
mod rendering {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn draw(snap: &TuiSnapshot, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let layout = TuiLayout::compute(width, height);
        terminal
            .draw(|frame| render(frame, snap, &layout))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect()
    }

    fn screen_contains(lines: &[String], fragment: &str) -> bool {
        lines.iter().any(|line| line.contains(fragment))
    }

    #[test]
    fn render_never_panics_for_sampled_sizes() {
        let mut snap = snapshot(64);
        for (w, h) in sample_sizes() {
            for mode in UiMode::ALL {
                snap.mode = mode;
                snap.ascii = w % 2 == 0;
                snap.learn = (mode == UiMode::MidiLearn).then(|| "track 1 gain".to_string());
                snap.command = (mode == UiMode::Command).then(|| CommandView {
                    buffer: "swing 0.2".to_string(),
                    completion: None,
                });
                let backend = TestBackend::new(w, h);
                let mut terminal = Terminal::new(backend).expect("terminal");
                let layout = TuiLayout::compute(w, h);
                terminal
                    .draw(|frame| render(frame, &snap, &layout))
                    .expect("draw");
            }
        }
    }

    #[test]
    fn render_tolerates_inconsistent_snapshots() {
        let mut snap = snapshot(8);
        snap.selected_track = 99;
        snap.grid.lock_counts.clear();
        snap.grid.playhead = Some(1_000);
        snap.grid.viewport = GridViewport::new(300, 16);
        snap.grid.viewport.set_cursor(299);
        snap.inspector.selected = 50;
        snap.transport.bpm = f64::NAN;
        snap.tracks[0].peak = f32::INFINITY;
        draw(&snap, 100, 30);
        snap.tracks.clear();
        draw(&snap, 100, 30);
    }

    #[test]
    fn compact_layout_warns_about_size() {
        let lines = draw(&snapshot(16), 40, 10);
        assert!(screen_contains(&lines, "terminal too small: need 60x18"));
        assert!(screen_contains(&lines, "128.0 BPM"));
    }

    #[test]
    fn transport_shows_tempo_position_and_scenes() {
        let lines = draw(&snapshot(16), 120, 30);
        assert!(screen_contains(&lines, "128.0 BPM"));
        assert!(screen_contains(&lines, "3.2.1"));
        assert!(screen_contains(&lines, "verse"));
        assert!(screen_contains(&lines, "chorus@5"));
        assert!(screen_contains(&lines, "Launchkey"));
        assert!(screen_contains(&lines, "default"));
    }

    #[test]
    fn track_rows_show_number_name_and_mute_solo() {
        let lines = draw(&snapshot(16), 100, 30);
        assert!(screen_contains(&lines, "01 DRUMS"));
        assert!(screen_contains(&lines, "02 BASS"));
        assert!(screen_contains(&lines, "03 LEAD"));
        let bass = lines.iter().find(|l| l.contains("02 BASS")).unwrap();
        assert!(bass.contains("M s"), "{bass}");
        let lead = lines.iter().find(|l| l.contains("03 LEAD")).unwrap();
        assert!(lead.contains("m S"), "{lead}");
    }

    #[test]
    fn ascii_mode_uses_hash_meters_and_ascii_grid() {
        let mut snap = snapshot(16);
        snap.ascii = true;
        snap.tracks[0].peak = 1.0;
        snap.grid.playhead = Some(2);
        let lines = draw(&snap, 100, 30);
        assert!(screen_contains(&lines, "####"));
        // Step 0 active with cursor, step 2 empty under playhead, step 4 active.
        assert!(screen_contains(&lines, "X.|.X...X...X..."), "{lines:#?}");
    }

    #[test]
    fn grid_shows_queued_versus_active_revision() {
        let mut snap = snapshot(16);
        snap.tracks[0].revision_state = RevisionState::Queued {
            active: 4,
            queued: 5,
        };
        let lines = draw(&snap, 100, 30);
        assert!(screen_contains(&lines, "rev 4 (queued 5)"));
        snap.tracks[0].revision_state = RevisionState::Active(4);
        let lines = draw(&snap, 100, 30);
        assert!(screen_contains(&lines, "rev 4"));
        assert!(!screen_contains(&lines, "queued 5"));
    }

    #[test]
    fn grid_shows_viewport_position_for_long_patterns() {
        let mut snap = snapshot(64);
        snap.grid.viewport.scroll(16);
        let lines = draw(&snap, 100, 30);
        assert!(screen_contains(&lines, "17-32/64"), "{lines:#?}");
    }

    #[test]
    fn inspector_and_status_and_command_line() {
        let mut snap = snapshot(16);
        let lines = draw(&snap, 100, 30);
        assert!(screen_contains(&lines, "Step 1"));
        assert!(screen_contains(&lines, "velocity"));
        assert!(screen_contains(&lines, "0.80"));
        assert!(screen_contains(&lines, "ready"));
        assert!(screen_contains(&lines, "XY"));

        snap.status.push(StatusLine {
            text: "bad thing".to_string(),
            error: true,
        });
        snap.mode = UiMode::Command;
        snap.command = Some(CommandView {
            buffer: "swi".to_string(),
            completion: Some("swing".to_string()),
        });
        let lines = draw(&snap, 100, 30);
        assert!(screen_contains(&lines, "error: bad thing"));
        assert!(screen_contains(&lines, ":swi"));
    }

    #[test]
    fn learn_modal_and_help_overlay() {
        let mut snap = snapshot(16);
        snap.mode = UiMode::MidiLearn;
        snap.learn = Some("LEAD cutoff".to_string());
        let lines = draw(&snap, 100, 30);
        assert!(screen_contains(&lines, "MIDI LEARN"));
        assert!(screen_contains(&lines, "LEAD cutoff"));

        snap.mode = UiMode::Help;
        snap.learn = None;
        let lines = draw(&snap, 100, 40);
        assert!(screen_contains(&lines, "Help"));
        assert!(screen_contains(&lines, "Ctrl+Q"));
    }

    #[test]
    fn crossterm_events_convert_to_ui_input() {
        use crossterm::event::{
            Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton,
            MouseEvent, MouseEventKind,
        };
        let press = Event::Key(KeyEvent::new(KeyCode::Char('z'), KeyModifiers::NONE));
        assert_eq!(ui_input_from_crossterm(&press), Some(UiInput::char('z')));
        let release = Event::Key(KeyEvent {
            code: KeyCode::Char('z'),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        });
        assert_eq!(
            ui_input_from_crossterm(&release),
            Some(UiInput::Key {
                key: UiKey::Char('z'),
                ctrl: false,
                alt: false,
                shift: false,
                release: true
            })
        );
        let ctrl_q = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert_eq!(
            route_input(
                UiMode::Command,
                &ui_input_from_crossterm(&ctrl_q).unwrap(),
                None,
                None
            ),
            vec![UiAction::Quit]
        );
        let f9 = Event::Key(KeyEvent::new(KeyCode::F(9), KeyModifiers::NONE));
        assert_eq!(
            ui_input_from_crossterm(&f9),
            Some(UiInput::key(UiKey::F(9)))
        );
        let click = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 4,
            row: 7,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(
            ui_input_from_crossterm(&click),
            Some(UiInput::Mouse {
                column: 4,
                row: 7,
                kind: UiMouseKind::Down
            })
        );
        let moved = Event::Mouse(MouseEvent {
            kind: MouseEventKind::Moved,
            column: 4,
            row: 7,
            modifiers: KeyModifiers::NONE,
        });
        assert_eq!(ui_input_from_crossterm(&moved), None);
        assert_eq!(
            ui_input_from_crossterm(&Event::Resize(80, 24)),
            Some(UiInput::Resize {
                width: 80,
                height: 24
            })
        );
        assert_eq!(ui_input_from_crossterm(&Event::FocusGained), None);
    }
}
