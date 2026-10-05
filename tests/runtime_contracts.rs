use shelloop::{
    parse_command, AxisCurve, AxisMapping, Command, RecordingQueue, Transport, TransportState,
    PerformanceMix, XyPoint,
};

#[test]
fn transport_only_advances_while_playing() {
    let mut transport = Transport::new(128.0);
    transport.advance(256);
    assert_eq!(transport.frame_position(), 0);

    transport.play();
    assert_eq!(transport.state(), TransportState::Playing);
    transport.advance(256);
    assert_eq!(transport.frame_position(), 256);

    transport.pause();
    transport.advance(256);
    assert_eq!(transport.frame_position(), 256);

    transport.stop();
    assert_eq!(transport.state(), TransportState::Stopped);
    assert_eq!(transport.frame_position(), 0);
}

#[test]
fn restart_returns_to_zero_and_plays() {
    let mut transport = Transport::new(120.0);
    transport.play();
    transport.advance(999);
    transport.restart();

    assert_eq!(transport.frame_position(), 0);
    assert_eq!(transport.state(), TransportState::Playing);
}

#[test]
fn tempo_change_rejects_unmusical_or_nonfinite_values() {
    let mut transport = Transport::new(120.0);
    assert!(transport.set_bpm(174.0).is_ok());
    assert_eq!(transport.bpm(), 174.0);
    assert!(transport.set_bpm(0.0).is_err());
    assert!(transport.set_bpm(f64::NAN).is_err());
}

#[test]
fn terminal_xy_is_normalized_and_y_points_up() {
    let point = XyPoint::from_terminal(50, 25, 101, 51);
    assert!((point.x - 0.5).abs() < 0.0001);
    assert!((point.y - 0.5).abs() < 0.0001);

    let top_right = XyPoint::from_terminal(100, 0, 101, 51);
    assert_eq!(top_right.x, 1.0);
    assert_eq!(top_right.y, 1.0);
}

#[test]
fn axis_mapping_supports_linear_log_and_inversion() {
    let linear = AxisMapping {
        min: 100.0,
        max: 1100.0,
        inverted: false,
        curve: AxisCurve::Linear,
    };
    assert!((linear.map(0.5) - 600.0).abs() < 0.001);

    let inverted = AxisMapping {
        inverted: true,
        ..linear
    };
    assert!((inverted.map(0.0) - 1100.0).abs() < 0.001);

    let log = AxisMapping {
        min: 100.0,
        max: 10_000.0,
        inverted: false,
        curve: AxisCurve::Logarithmic,
    };
    assert!((log.map(0.5) - 1000.0).abs() < 0.01);
}

#[test]
fn recording_queue_is_bounded_and_reports_drops() {
    let mut queue = RecordingQueue::new(2);
    assert!(queue.push(vec![1.0, 2.0]));
    assert!(queue.push(vec![3.0, 4.0]));
    assert!(!queue.push(vec![5.0, 6.0]));
    assert_eq!(queue.len(), 2);
    assert_eq!(queue.dropped_blocks(), 1);
    assert_eq!(queue.pop(), Some(vec![1.0, 2.0]));
}

#[test]
fn command_parser_handles_transport_tempo_and_quoted_paths() {
    assert_eq!(parse_command("play").unwrap(), Command::Play);
    assert_eq!(parse_command("tempo 174").unwrap(), Command::Tempo(174.0));
    assert_eq!(
        parse_command("save \"sets/night one.json\"").unwrap(),
        Command::Save("sets/night one.json".into())
    );
    assert!(parse_command("tempo nope").is_err());
    assert!(parse_command("unknown thing").is_err());
}

#[test]
fn mouse_xy_maps_crossfade_and_master_level() {
    let bottom_left = PerformanceMix::from_xy(XyPoint { x: 0.0, y: 0.0 }, true);
    assert_eq!(bottom_left.live_gain, 0.0);
    assert_eq!(bottom_left.sequencer_gain, 0.0);

    let top_left = PerformanceMix::from_xy(XyPoint { x: 0.0, y: 1.0 }, true);
    assert_eq!(top_left.live_gain, 1.0);
    assert_eq!(top_left.sequencer_gain, 0.0);

    let top_right = PerformanceMix::from_xy(XyPoint { x: 1.0, y: 1.0 }, true);
    assert_eq!(top_right.live_gain, 0.0);
    assert_eq!(top_right.sequencer_gain, 1.0);

    let no_pattern = PerformanceMix::from_xy(XyPoint { x: 1.0, y: 0.5 }, false);
    assert_eq!(no_pattern.live_gain, 0.5);
    assert_eq!(no_pattern.sequencer_gain, 0.0);
}
