//! Spec 17 acceptance: bitcrusher effect kind.

use shelloop::{effect_params, find_effect_param, Effect, EffectConfig, EffectKind, EffectParamId};

const SR: u32 = 48_000;

fn build(params: &[(&str, f32)]) -> Effect {
    let mut config = EffectConfig::new(EffectKind::BitCrusher);
    for (name, value) in params {
        let id = find_effect_param(EffectKind::BitCrusher, name).unwrap();
        config.set_param_value(id, *value).expect("valid value");
    }
    Effect::new(&config, SR, 120.0).expect("effect builds")
}

fn set(effect: &mut Effect, name: &str, value: f32) {
    let id = find_effect_param(EffectKind::BitCrusher, name).unwrap();
    assert!(effect.set_param(id, value));
}

fn ramp_input(n: usize) -> f32 {
    ((n as f32) * 0.0137).sin() * 0.9
}

#[test]
fn name_parameters_and_defaults_are_stable() {
    assert_eq!(
        EffectKind::parse("bitcrusher"),
        Some(EffectKind::BitCrusher)
    );
    assert_eq!(EffectKind::BitCrusher.name(), "bitcrusher");
    let names: Vec<_> = effect_params(EffectKind::BitCrusher)
        .iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(names, ["bits", "downsample", "mix"]);
    assert_eq!(
        find_effect_param(EffectKind::BitCrusher, "mix"),
        Some(EffectParamId(2))
    );
    assert!(EffectConfig::new(EffectKind::BitCrusher)
        .set_param_value(EffectParamId(0), 0.0)
        .is_err());
    assert!(EffectConfig::new(EffectKind::BitCrusher)
        .set_param_value(EffectParamId(1), 33.0)
        .is_err());
    assert_eq!(
        EffectConfig::new(EffectKind::BitCrusher).required_memory_bytes(SR),
        0,
        "bitcrusher uses no buffers"
    );
}

#[test]
fn sixteen_bits_without_downsampling_is_nearly_transparent() {
    let mut fx = build(&[("bits", 16.0), ("downsample", 1.0)]);
    for n in 0..4_000 {
        let x = ramp_input(n);
        let (l, r) = fx.process(x, -x);
        assert!((l - x).abs() <= 2f32.powi(-15), "n={n}: {l} vs {x}");
        assert!((r + x).abs() <= 2f32.powi(-15));
    }
}

#[test]
fn one_bit_only_produces_three_levels() {
    let mut fx = build(&[("bits", 1.0), ("downsample", 1.0)]);
    for n in 0..4_000 {
        let (l, _) = fx.process(ramp_input(n), 0.0);
        assert!(
            l == -1.0 || l == 0.0 || l == 1.0,
            "unexpected level {l} at {n}"
        );
    }
}

#[test]
fn downsampling_holds_each_value_for_exactly_n_frames() {
    let mut fx = build(&[("bits", 16.0), ("downsample", 5.0)]);
    let outputs: Vec<f32> = (0..200).map(|n| fx.process(ramp_input(n), 0.0).0).collect();
    for block in outputs.chunks(5) {
        assert!(block.iter().all(|v| *v == block[0]), "held block {block:?}");
    }
    // Consecutive blocks differ for a moving input.
    assert!(outputs
        .chunks(5)
        .collect::<Vec<_>>()
        .windows(2)
        .any(|w| w[0][0] != w[1][0]));
}

#[test]
fn mix_zero_is_dry_and_reset_clears_held_state() {
    let mut fx = build(&[("bits", 2.0), ("downsample", 16.0), ("mix", 0.0)]);
    for n in 0..500 {
        let x = ramp_input(n);
        let (l, _) = fx.process(x, 0.0);
        assert!((l - x).abs() < 1e-6, "dry at {n}");
    }
    set(&mut fx, "mix", 1.0);
    for n in 0..2_000 {
        fx.process(ramp_input(n), 0.0);
    }
    fx.reset();
    for _ in 0..2_000 {
        let (l, r) = fx.process(0.0, 0.0);
        assert!(l == 0.0 && r == 0.0, "stale hold after reset");
    }
}

#[test]
fn output_stays_finite_and_channels_are_independent() {
    let mut fx = build(&[("bits", 4.0), ("downsample", 3.0)]);
    for n in 0..5_000 {
        let (l, r) = fx.process(if n % 7 == 0 { f32::NAN } else { 0.5 }, 0.25);
        assert!(l.is_finite() && r.is_finite());
    }
    let mut mono = build(&[("bits", 4.0), ("downsample", 1.0)]);
    let (_, right_a) = mono.process(0.3, 0.6);
    let mut other = build(&[("bits", 4.0), ("downsample", 1.0)]);
    let (_, right_b) = other.process(-0.9, 0.6);
    assert_eq!(
        right_a, right_b,
        "right output does not depend on left input"
    );
}
