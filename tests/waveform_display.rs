use shelloop::scope::{
    draw_panel_sequence, fit_width, header_line, open_panel_sequence, peak_dbfs, raw_mode_line,
    release_panel_sequence, render_scope, scope_window, trigger_start, AXIS_CHAR, WAVE_CHAR,
};
use shelloop::{PanelLayout, PeakHistory, ScopeTap, WaveformStyle, SCOPE_CAPACITY};
use std::sync::Arc;
use std::thread;

fn sine(len: usize, period: usize, amplitude: f32) -> Vec<f32> {
    (0..len)
        .map(|index| amplitude * (std::f32::consts::TAU * index as f32 / period as f32).sin())
        .collect()
}

#[test]
fn disabled_tap_ignores_audio_so_the_callback_cost_is_one_load() {
    let tap = ScopeTap::new();
    tap.push(0.5);
    assert_eq!(tap.written(), 0);
    tap.set_enabled(true);
    tap.push(0.5);
    assert_eq!(tap.written(), 1);
}

#[test]
fn tap_returns_latest_samples_oldest_first() {
    let tap = ScopeTap::new();
    tap.set_enabled(true);
    for index in 0..10 {
        tap.push(index as f32 / 10.0);
    }
    let mut out = Vec::new();
    tap.copy_latest(3, &mut out);
    assert_eq!(out, vec![0.7, 0.8, 0.9]);
    tap.copy_latest(100, &mut out);
    assert_eq!(out.len(), 10);
}

#[test]
fn tap_sanitises_non_finite_and_out_of_range_samples() {
    let tap = ScopeTap::new();
    tap.set_enabled(true);
    for sample in [f32::NAN, f32::INFINITY, 3.0, -3.0] {
        tap.push(sample);
    }
    let mut out = Vec::new();
    tap.copy_latest(4, &mut out);
    assert_eq!(out, vec![0.0, 0.0, 1.0, -1.0]);
}

#[test]
fn tap_wraps_without_growing_and_reader_catches_up_after_falling_behind() {
    let tap = ScopeTap::new();
    tap.set_enabled(true);
    let mut cursor = 0;
    let mut out = Vec::new();
    for index in 0..(SCOPE_CAPACITY * 3) {
        tap.push((index % 7) as f32 / 10.0);
    }
    tap.copy_since(&mut cursor, &mut out);
    assert_eq!(out.len(), SCOPE_CAPACITY);
    assert_eq!(cursor, (SCOPE_CAPACITY * 3) as u64);
    assert_eq!(
        *out.last().unwrap(),
        ((SCOPE_CAPACITY * 3 - 1) % 7) as f32 / 10.0
    );

    tap.push(0.25);
    tap.copy_since(&mut cursor, &mut out);
    assert_eq!(out, vec![0.25]);
    tap.copy_since(&mut cursor, &mut out);
    assert!(out.is_empty());
}

#[test]
fn tap_can_be_written_and_read_from_different_threads() {
    let tap = Arc::new(ScopeTap::new());
    tap.set_enabled(true);
    let writer = {
        let tap = Arc::clone(&tap);
        thread::spawn(move || {
            for index in 0..100_000 {
                tap.push((index % 100) as f32 / 100.0);
            }
        })
    };
    let mut cursor = 0;
    let mut out = Vec::new();
    let mut total = 0_u64;
    while !writer.is_finished() {
        tap.copy_since(&mut cursor, &mut out);
        assert!(out.iter().all(|sample| (0.0..1.0).contains(sample)));
        total += out.len() as u64;
    }
    writer.join().unwrap();
    tap.copy_since(&mut cursor, &mut out);
    total += out.len() as u64;
    assert_eq!(cursor, 100_000);
    assert!(total <= 100_000);
}

#[test]
fn scope_render_has_exact_dimensions_and_a_flat_axis_for_silence() {
    let lines = render_scope(&[0.0; 512], 40, 9);
    assert_eq!(lines.len(), 9);
    assert!(lines.iter().all(|line| line.chars().count() == 40));
    // Silence collapses to the centre row; everything else is blank.
    for (row, line) in lines.iter().enumerate() {
        if row == 4 {
            assert!(line.chars().all(|c| c == WAVE_CHAR), "row {row}: {line:?}");
        } else {
            assert!(line.chars().all(|c| c == ' '), "row {row}: {line:?}");
        }
    }
}

#[test]
fn empty_scope_shows_only_the_axis() {
    let lines = render_scope(&[], 10, 5);
    assert_eq!(lines[2], AXIS_CHAR.to_string().repeat(10));
    assert_eq!(lines[0], " ".repeat(10));
}

#[test]
fn loud_sine_reaches_the_top_and_bottom_rows() {
    let lines = render_scope(&sine(1024, 128, 0.9), 64, 11);
    assert!(lines[0].contains(WAVE_CHAR));
    assert!(lines[10].contains(WAVE_CHAR));
}

#[test]
fn quiet_signal_is_drawn_smaller_than_a_loud_one() {
    let quiet = render_scope(&sine(1024, 128, 0.05), 64, 11);
    assert!(!quiet[0].contains(WAVE_CHAR));
    assert!(!quiet[10].contains(WAVE_CHAR));
    assert!(quiet[5].contains(WAVE_CHAR));
}

#[test]
fn scope_render_handles_fewer_samples_than_columns() {
    let lines = render_scope(&[0.5, -0.5, 0.5], 30, 7);
    assert_eq!(lines.len(), 7);
    assert!(lines.iter().all(|line| line.chars().count() == 30));
}

#[test]
fn scope_render_ignores_non_finite_values() {
    let lines = render_scope(&[f32::NAN, f32::INFINITY, 0.0], 3, 5);
    assert_eq!(lines.len(), 5);
}

#[test]
fn trigger_lands_on_a_rising_zero_crossing_near_the_newest_data() {
    let samples = sine(4000, 100, 0.8);
    let start = trigger_start(&samples, 1000);
    assert!(start <= 3000);
    assert!(
        start > 2800,
        "start {start} should be near the newest window"
    );
    assert!(samples[start - 1] < 0.0 && samples[start] >= 0.0);
}

#[test]
fn trigger_falls_back_to_the_newest_window() {
    assert_eq!(trigger_start(&[0.5; 100], 40), 60);
    assert_eq!(trigger_start(&[0.5; 10], 40), 0);
}

#[test]
fn scope_window_tracks_sample_rate_and_fits_the_tap() {
    assert_eq!(scope_window(48_000), 1920);
    assert_eq!(scope_window(44_100), 1764);
    assert!(scope_window(u32::MAX) <= SCOPE_CAPACITY / 2);
    assert!(scope_window(1) >= 64);
}

#[test]
fn history_scrolls_left_and_keeps_only_the_newest_columns() {
    let mut history = PeakHistory::new(4);
    for level in [0.1, 0.2, 0.3, 0.4, 0.9] {
        history.push_block(&[level, -level]);
    }
    assert_eq!(history.len(), 4);
    assert!((history.latest() - 0.9).abs() < 1e-6);

    let lines = history.render(6, 9);
    // Columns 0-1 have no data yet; the newest peak is the right-most column
    // and spans the whole height.
    assert!(lines.iter().all(|line| !line[..2].contains(WAVE_CHAR)));
    assert!(lines.iter().all(|line| line.ends_with(WAVE_CHAR)));
}

#[test]
fn history_resize_keeps_the_newest_columns() {
    let mut history = PeakHistory::new(10);
    for level in 0..10 {
        history.push_block(&[level as f32 / 10.0]);
    }
    history.resize(3);
    assert_eq!(history.len(), 3);
    assert!((history.latest() - 0.9).abs() < 1e-6);
    history.clear();
    assert!(history.is_empty());
}

#[test]
fn panel_layout_reserves_the_bottom_rows_and_refuses_tiny_terminals() {
    let layout = PanelLayout::for_terminal(80, 30).unwrap();
    assert_eq!(layout.rows, 10);
    assert_eq!(layout.scroll_bottom, 20);
    assert_eq!(layout.first_row, 21);
    assert_eq!(layout.wave_rows(), 9);

    let tall = PanelLayout::for_terminal(80, 200).unwrap();
    assert_eq!(tall.rows, 16);
    assert_eq!(tall.first_row + tall.rows - 1, 200);

    let short = PanelLayout::for_terminal(80, 12).unwrap();
    assert_eq!(short.rows, 6);
    assert_eq!(short.scroll_bottom, 6);

    assert!(PanelLayout::for_terminal(80, 11).is_none());
    assert!(PanelLayout::for_terminal(19, 40).is_none());
}

#[test]
fn panel_sequences_restore_the_cursor_and_address_only_panel_rows() {
    let layout = PanelLayout::for_terminal(40, 24).unwrap();
    let lines = vec!["abc".to_string(); usize::from(layout.rows) + 3];

    let draw = draw_panel_sequence(&layout, &lines);
    assert!(draw.starts_with("\x1b7") && draw.ends_with("\x1b8"));
    assert_eq!(draw.matches("H\x1b[2K").count(), usize::from(layout.rows));
    assert!(draw.contains(&format!("\x1b[{};1H", layout.first_row)));
    assert!(!draw.contains(&format!("\x1b[{};1H", layout.first_row - 1)));

    let open = open_panel_sequence(&layout);
    assert!(open.contains(&format!("\x1b[1;{}r", layout.scroll_bottom)));
    assert_eq!(open.matches("\r\n").count(), usize::from(layout.rows));

    let release = release_panel_sequence(&layout);
    assert!(release.starts_with("\x1b7\x1b[r"));
    assert!(release.ends_with("\x1b8"));
}

#[test]
fn header_reports_style_and_level_at_exact_width() {
    let line = header_line(WaveformStyle::History, -6.02, 80);
    assert_eq!(line.chars().count(), 80);
    assert!(line.contains("history"));
    assert!(line.contains("-6.0 dBFS"));
    assert_eq!(
        header_line(WaveformStyle::Scope, -200.0, 20)
            .chars()
            .count(),
        20
    );
    assert_eq!(fit_width("abcdef", 3, '-'), "abc");
    assert_eq!(fit_width("ab", 4, '-'), "ab--");
}

#[test]
fn peak_dbfs_is_finite_for_silence() {
    assert_eq!(peak_dbfs(&[0.0; 16]), -96.0);
    assert!((peak_dbfs(&[0.5, -1.0]) - 0.0).abs() < 1e-4);
}

#[test]
fn style_cycles_and_parses() {
    assert_eq!(WaveformStyle::Scope.next(), WaveformStyle::History);
    assert_eq!(WaveformStyle::History.next(), WaveformStyle::Scope);
    assert_eq!(
        WaveformStyle::parse("history"),
        Some(WaveformStyle::History)
    );
    assert_eq!(WaveformStyle::parse("scope"), Some(WaveformStyle::Scope));
    assert_eq!(WaveformStyle::parse("bars"), None);
}

#[test]
fn raw_mode_lines_return_the_carriage() {
    assert_eq!(raw_mode_line("edit: ok"), "\r\x1b[2Kedit: ok\r\n");
}
