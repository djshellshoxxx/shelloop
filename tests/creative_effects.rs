//! Spec 15 acceptance tests: filter, chorus/flanger and beat repeat.

use shelloop::{
    effect_params, find_effect_param, Effect, EffectConfig, EffectKind, EffectParamId, EffectRack,
    EffectsConfig,
};

fn build(kind: EffectKind, params: &[(&str, f32)], sample_rate: u32, bpm: f64) -> Effect {
    let mut config = EffectConfig::new(kind);
    for (name, value) in params {
        let id = find_effect_param(kind, name).unwrap_or_else(|| panic!("no `{name}`"));
        config.set_param_value(id, *value).expect("valid value");
    }
    Effect::new(&config, sample_rate, bpm).expect("effect builds")
}

fn set(effect: &mut Effect, kind: EffectKind, name: &str, value: f32) {
    let id = find_effect_param(kind, name).unwrap();
    assert!(effect.set_param(id, value));
}

fn sine(n: usize, freq: f32, sample_rate: f32) -> f32 {
    (n as f32 * freq * std::f32::consts::TAU / sample_rate).sin()
}

fn rms_of(effect: &mut Effect, freq: f32, sample_rate: f32) -> f32 {
    let frames = 8_000;
    let mut sum = 0.0;
    for n in 0..frames {
        let x = sine(n, freq, sample_rate);
        let (l, _) = effect.process(x, x);
        if n >= frames / 2 {
            sum += l * l;
        }
    }
    (sum / (frames / 2) as f32).sqrt()
}

#[test]
fn new_effects_have_stable_ids_and_valid_descriptors() {
    for (kind, names) in [
        (
            EffectKind::Filter,
            vec![
                "mode",
                "cutoff",
                "resonance",
                "env_amount",
                "sync",
                "depth",
                "mix",
            ],
        ),
        (
            EffectKind::Chorus,
            vec![
                "mode", "rate_hz", "sync", "depth", "feedback", "width", "mix",
            ],
        ),
        (
            EffectKind::BeatRepeat,
            vec![
                "division", "repeats", "reverse", "gate", "mix", "active", "seed",
            ],
        ),
    ] {
        let got: Vec<_> = effect_params(kind).iter().map(|d| d.name).collect();
        assert_eq!(got, names);
        assert_eq!(EffectKind::parse(kind.name()), Some(kind));
        for (index, d) in effect_params(kind).iter().enumerate() {
            assert_eq!(
                find_effect_param(kind, d.name),
                Some(EffectParamId(index as u8))
            );
            assert!(d.min <= d.default && d.default <= d.max);
        }
    }
    let feedback = &effect_params(EffectKind::Chorus)[4];
    assert!(feedback.max < 1.0 && feedback.min > -1.0);
}

#[test]
fn filter_modes_pass_and_reject_the_right_bands() {
    let sr = 48_000.0;
    let mut lp = build(
        EffectKind::Filter,
        &[("mode", 0.0), ("cutoff", 1_000.0)],
        48_000,
        120.0,
    );
    let mut hp = build(
        EffectKind::Filter,
        &[("mode", 2.0), ("cutoff", 1_000.0)],
        48_000,
        120.0,
    );
    let mut bp = build(
        EffectKind::Filter,
        &[("mode", 1.0), ("cutoff", 1_000.0), ("resonance", 0.6)],
        48_000,
        120.0,
    );
    assert!(
        rms_of(&mut lp, 100.0, sr)
            > 5.0
                * rms_of(
                    &mut build(
                        EffectKind::Filter,
                        &[("mode", 0.0), ("cutoff", 1_000.0)],
                        48_000,
                        120.0
                    ),
                    8_000.0,
                    sr
                )
    );
    assert!(
        rms_of(&mut hp, 8_000.0, sr)
            > 5.0
                * rms_of(
                    &mut build(
                        EffectKind::Filter,
                        &[("mode", 2.0), ("cutoff", 1_000.0)],
                        48_000,
                        120.0
                    ),
                    100.0,
                    sr
                )
    );
    let centre = rms_of(&mut bp, 1_000.0, sr);
    let mut bp2 = build(
        EffectKind::Filter,
        &[("mode", 1.0), ("cutoff", 1_000.0), ("resonance", 0.6)],
        48_000,
        120.0,
    );
    assert!(centre > 2.0 * rms_of(&mut bp2, 60.0, sr));
}

#[test]
fn filter_mix_zero_is_exactly_dry_and_max_resonance_is_finite() {
    let mut dry = build(
        EffectKind::Filter,
        &[("mix", 0.0), ("cutoff", 300.0)],
        44_100,
        120.0,
    );
    let mut wild = build(
        EffectKind::Filter,
        &[
            ("resonance", 0.95),
            ("cutoff", 400.0),
            ("env_amount", 1.0),
            ("sync", 2.0),
            ("depth", 1.0),
        ],
        44_100,
        200.0,
    );
    for n in 0..20_000 {
        let x = sine(n, 440.0, 44_100.0) * 0.5;
        let (l, r) = dry.process(x, -x);
        assert_eq!((l, r), (x, -x));
        let (l, r) = wild.process(x * 4.0, -x * 4.0);
        assert!(l.is_finite() && r.is_finite() && l.abs() < 200.0 && r.abs() < 200.0);
    }
}

#[test]
fn filter_sweep_follows_tempo_and_is_deterministic() {
    let run = |bpm: f64| {
        let mut fx = build(
            EffectKind::Filter,
            &[
                ("sync", 1.0),
                ("depth", 1.0),
                ("cutoff", 1_500.0),
                ("mode", 0.0),
            ],
            48_000,
            bpm,
        );
        (0..24_000)
            .map(|n| fx.process(sine(n, 3_000.0, 48_000.0), 0.0).0)
            .collect::<Vec<_>>()
    };
    assert_eq!(run(120.0), run(120.0));
    assert_ne!(run(120.0), run(90.0));
    // set_tempo re-derives the sweep rate to match a fresh instance.
    let mut fx = build(
        EffectKind::Filter,
        &[("sync", 1.0), ("depth", 1.0), ("cutoff", 1_500.0)],
        48_000,
        90.0,
    );
    fx.set_tempo(120.0);
    let retuned: Vec<_> = (0..24_000)
        .map(|n| fx.process(sine(n, 3_000.0, 48_000.0), 0.0).0)
        .collect();
    assert_eq!(retuned, run(120.0));
}

#[test]
fn flanger_impulse_arrives_at_the_centre_delay_and_channels_stay_isolated() {
    let sr = 48_000;
    let mut fx = build(
        EffectKind::Chorus,
        &[
            ("mode", 1.0),
            ("depth", 0.0),
            ("mix", 1.0),
            ("feedback", 0.0),
        ],
        sr,
        120.0,
    );
    let mut out = Vec::new();
    for n in 0..400 {
        out.push(fx.process(if n == 0 { 1.0 } else { 0.0 }, 0.0));
    }
    let peak = out
        .iter()
        .enumerate()
        .max_by(|a, b| a.1 .0.abs().total_cmp(&b.1 .0.abs()))
        .unwrap();
    assert_eq!(peak.0, (0.003 * sr as f32) as usize);
    assert!((peak.1 .0 - 1.0).abs() < 1e-4);
    assert!(
        out.iter().all(|(_, r)| *r == 0.0),
        "left input must not leak right"
    );
}

#[test]
fn chorus_silence_in_silence_out_and_max_feedback_decays() {
    let mut fx = build(
        EffectKind::Chorus,
        &[("feedback", 0.9), ("mix", 1.0)],
        48_000,
        120.0,
    );
    for _ in 0..10_000 {
        assert_eq!(fx.process(0.0, 0.0), (0.0, 0.0));
    }
    fx.process(1.0, 1.0);
    let mut peak_late: f32 = 0.0;
    for n in 0..480_000 {
        let (l, r) = fx.process(0.0, 0.0);
        assert!(l.is_finite() && r.is_finite() && l.abs() <= 1.0);
        if n > 400_000 {
            peak_late = peak_late.max(l.abs()).max(r.abs());
        }
    }
    assert!(
        peak_late < 1e-3,
        "feedback tail must decay, got {peak_late}"
    );
}

#[test]
fn chorus_sync_rate_tracks_tempo() {
    let mut a = build(
        EffectKind::Chorus,
        &[("sync", 1.0), ("mix", 1.0)],
        48_000,
        120.0,
    );
    let mut b = build(
        EffectKind::Chorus,
        &[("sync", 1.0), ("mix", 1.0)],
        48_000,
        60.0,
    );
    let sig = |n: usize| sine(n, 700.0, 48_000.0);
    let oa: Vec<f32> = (0..30_000).map(|n| a.process(sig(n), sig(n)).0).collect();
    let ob: Vec<f32> = (0..30_000).map(|n| b.process(sig(n), sig(n)).0).collect();
    assert_ne!(oa, ob);
    b.set_tempo(120.0);
    b.reset();
    a.reset();
    let oa: Vec<f32> = (0..30_000).map(|n| a.process(sig(n), sig(n)).0).collect();
    let ob: Vec<f32> = (0..30_000).map(|n| b.process(sig(n), sig(n)).0).collect();
    assert_eq!(oa, ob);
}

const SR: u32 = 8_000;

fn ramp(n: usize) -> f32 {
    ((n * 37 + 11) % 997) as f32 / 997.0 - 0.5
}

fn repeat_fx(params: &[(&str, f32)]) -> Effect {
    // 120 BPM, division 1 (1/8) => 0.25 s => 2000 frames at 8 kHz.
    build(EffectKind::BeatRepeat, params, SR, 120.0)
}

fn run(fx: &mut Effect, from: usize, to: usize) -> Vec<f32> {
    (from..to).map(|n| fx.process(ramp(n), ramp(n)).0).collect()
}

#[test]
fn beat_repeat_never_plays_before_a_full_slice_is_captured() {
    let mut fx = repeat_fx(&[("active", 1.0)]);
    let out = run(&mut fx, 0, 1_999);
    for (n, v) in out.iter().enumerate() {
        assert_eq!(
            *v,
            ramp(n),
            "frame {n} must be dry before capture completes"
        );
    }
}

#[test]
fn beat_repeat_loops_the_captured_slice_on_the_grid() {
    let mut fx = repeat_fx(&[]);
    let before = run(&mut fx, 0, 100);
    assert!(before.iter().enumerate().all(|(n, v)| *v == ramp(n)));
    set(&mut fx, EffectKind::BeatRepeat, "active", 1.0);
    let out = run(&mut fx, 100, 6_100);
    for k in 100..1_900 {
        // first pass of the repeat equals the slice captured before the boundary
        let got = out[2_000 + k - 100];
        assert!((got - ramp(k)).abs() < 1e-5, "pass 1 k={k}");
        let got = out[4_000 + k - 100];
        assert!((got - ramp(k)).abs() < 1e-5, "pass 2 k={k}");
    }
    // Dry audio continues untouched before the boundary.
    for n in 100..2_000 {
        assert_eq!(out[n - 100], ramp(n));
    }
}

#[test]
fn beat_repeat_reverse_and_repeat_count_and_release() {
    let mut fx = repeat_fx(&[("reverse", 1.0), ("repeats", 2.0)]);
    run(&mut fx, 0, 10);
    set(&mut fx, EffectKind::BeatRepeat, "active", 1.0);
    let out = run(&mut fx, 10, 8_010);
    for k in 100..1_900 {
        let got = out[2_000 - 10 + k];
        assert!((got - ramp(1_999 - k)).abs() < 1e-5, "reverse k={k}");
    }
    // After two passes the effect releases back to the dry signal.
    for n in 6_100..8_010 {
        assert!((out[n - 10] - ramp(n)).abs() < 1e-5, "released n={n}");
    }
}

#[test]
fn beat_repeat_gate_is_seeded_and_full_gate_silences_repeats() {
    let render = |seed: f32, gate: f32| {
        let mut fx = repeat_fx(&[("gate", gate), ("seed", seed), ("division", 3.0)]);
        run(&mut fx, 0, 50);
        set(&mut fx, EffectKind::BeatRepeat, "active", 1.0);
        run(&mut fx, 50, 12_050)
    };
    assert_eq!(render(7.0, 0.5), render(7.0, 0.5));
    assert_ne!(render(7.0, 0.5), render(8.0, 0.5));
    let silent = render(7.0, 1.0);
    assert!(silent[3_000..].iter().all(|v| v.abs() < 1e-6));
}

#[test]
fn beat_repeat_reset_discards_captured_audio() {
    let mut fx = repeat_fx(&[("active", 1.0)]);
    run(&mut fx, 0, 4_500);
    fx.reset();
    // Silence after a reset: nothing stale may be replayed.
    let out: Vec<f32> = (0..1_999).map(|_| fx.process(0.0, 0.0).0).collect();
    assert!(out.iter().all(|v| *v == 0.0));
    let after: Vec<f32> = (0..2_100).map(|_| fx.process(0.0, 0.0).0).collect();
    assert!(after.iter().all(|v| *v == 0.0));
}

#[test]
fn creative_effects_chain_inside_a_rack_without_allocating_budget_surprises() {
    let mut config = EffectsConfig::default();
    for kind in [
        EffectKind::Filter,
        EffectKind::Chorus,
        EffectKind::BeatRepeat,
        EffectKind::BitCrusher,
    ] {
        config.master.push(EffectConfig::new(kind));
    }
    config.validate().unwrap();
    assert!(config.required_memory_bytes(48_000) > 0);
    let mut rack = EffectRack::new(&config.master, 4, 48_000, 120.0).unwrap();
    for n in 0..5_000 {
        let (l, r) = rack.process(ramp(n), ramp(n));
        assert!(l.is_finite() && r.is_finite());
    }
}
