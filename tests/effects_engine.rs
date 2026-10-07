use shelloop::{
    check_effect_memory, effect_params, find_effect_param, validate_track_inserts, Effect,
    EffectConfig, EffectKind, EffectParamId, EffectRack, EffectSlotId, EffectsConfig, ParamCurve,
    SendBusConfig, DEFAULT_EFFECT_MEMORY_BUDGET, MAX_DELAY_SECONDS, MAX_MASTER_INSERTS,
    MAX_SEND_BUSES, MAX_SEND_EFFECTS, MAX_TRACK_INSERTS,
};

const KINDS: [EffectKind; 3] = [
    EffectKind::Saturation,
    EffectKind::Delay,
    EffectKind::Reverb,
];

fn param(kind: EffectKind, name: &str) -> EffectParamId {
    find_effect_param(kind, name).unwrap_or_else(|| panic!("{kind:?} has no `{name}`"))
}

fn config(kind: EffectKind, params: &[(&str, f32)]) -> EffectConfig {
    let mut config = EffectConfig::new(kind);
    for (name, value) in params {
        config
            .set_param_value(param(kind, name), *value)
            .expect("valid parameter");
    }
    config
}

fn effect(kind: EffectKind, params: &[(&str, f32)], sample_rate: u32) -> Effect {
    Effect::new(&config(kind, params), sample_rate, 120.0).expect("effect builds")
}

/// Deterministic test signal: two detuned sines plus a little pseudo-noise.
fn signal(n: usize) -> (f32, f32) {
    let t = n as f32;
    let noise = ((n.wrapping_mul(2_654_435_761) >> 7) & 0xffff) as f32 / 65_535.0 - 0.5;
    (
        0.6 * (t * 0.031).sin() + 0.2 * noise,
        0.6 * (t * 0.017).sin() - 0.2 * noise,
    )
}

fn impulse_response(effect: &mut Effect, frames: usize) -> Vec<f32> {
    (0..frames)
        .map(|n| effect.process(if n == 0 { 1.0 } else { 0.0 }, 0.0).0)
        .collect()
}

#[test]
fn descriptor_tables_are_stable_and_named_in_snake_case() {
    let names: Vec<_> = effect_params(EffectKind::Saturation)
        .iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(names, ["mode", "drive", "tone", "mix", "output"]);
    let names: Vec<_> = effect_params(EffectKind::Delay)
        .iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(
        names,
        ["time_ms", "sync", "feedback", "mix", "ping_pong", "damping"]
    );
    let names: Vec<_> = effect_params(EffectKind::Reverb)
        .iter()
        .map(|d| d.name)
        .collect();
    assert_eq!(
        names,
        ["size", "decay", "damping", "predelay_ms", "mix", "width"]
    );

    for kind in KINDS {
        assert_eq!(EffectKind::parse(kind.name()), Some(kind));
        for (index, descriptor) in effect_params(kind).iter().enumerate() {
            assert_eq!(
                find_effect_param(kind, descriptor.name),
                Some(EffectParamId(index as u8))
            );
            assert!(descriptor.min <= descriptor.default && descriptor.default <= descriptor.max);
            assert_eq!(
                descriptor.lockable,
                descriptor.curve != ParamCurve::Discrete,
                "{}",
                descriptor.name
            );
        }
    }
    assert_eq!(EffectKind::parse("chorus"), None);
    assert_eq!(find_effect_param(EffectKind::Delay, "drive"), None);
}

#[test]
fn every_effect_is_finite_at_parameter_extremes_and_for_nan_input() {
    for kind in KINDS {
        let descriptors = effect_params(kind);
        let combos = 1usize << descriptors.len();
        for mask in 0..combos {
            let mut config = EffectConfig::new(kind);
            for (index, descriptor) in descriptors.iter().enumerate() {
                let value = if mask & (1 << index) == 0 {
                    descriptor.min
                } else {
                    descriptor.max
                };
                config
                    .set_param_value(EffectParamId(index as u8), value)
                    .unwrap();
            }
            let mut effect = Effect::new(&config, 8_000, 300.0).unwrap();
            for n in 0..4_000 {
                let (l, r) = signal(n);
                let (l, r) = effect.process(l * 4.0, r * 4.0);
                assert!(l.is_finite() && r.is_finite(), "{kind:?} mask {mask:b}");
            }
            for input in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, f32::MAX] {
                let (l, r) = effect.process(input, -input);
                assert!(l.is_finite() && r.is_finite(), "{kind:?} input {input}");
            }
            for _ in 0..200 {
                let (l, r) = effect.process(0.0, 0.0);
                assert!(l.is_finite() && r.is_finite());
            }
        }
    }
}

#[test]
fn saturation_mix_zero_is_dry_and_output_gain_endpoints_hold() {
    let mut dry = effect(
        EffectKind::Saturation,
        &[("drive", 36.0), ("mix", 0.0), ("mode", 1.0)],
        48_000,
    );
    for n in 0..2_000 {
        let (l, r) = signal(n);
        assert_eq!(dry.process(l, r), (l, r));
    }

    let mut silent = effect(EffectKind::Saturation, &[("output", 0.0)], 48_000);
    for n in 0..500 {
        let (l, r) = signal(n);
        assert_eq!(silent.process(l, r), (0.0, 0.0));
    }

    let mut doubled = effect(
        EffectKind::Saturation,
        &[("mix", 0.0), ("output", 2.0)],
        48_000,
    );
    for n in 0..500 {
        let (l, r) = signal(n);
        assert_eq!(doubled.process(l, r), (l * 2.0, r * 2.0));
    }

    // Fully wet hard clip at maximum drive stays inside the unit range.
    let mut clipped = effect(
        EffectKind::Saturation,
        &[("mode", 1.0), ("drive", 36.0)],
        48_000,
    );
    for n in 0..2_000 {
        let (l, r) = signal(n);
        let (l, r) = clipped.process(l, r);
        assert!(l.abs() <= 1.0 && r.abs() <= 1.0);
    }
}

#[test]
fn delay_impulse_appears_at_the_rounded_sample_index() {
    for (time_ms, sample_rate) in [(375.0, 48_000), (10.3, 44_100), (1.0, 48_000)] {
        let mut delay = effect(
            EffectKind::Delay,
            &[("time_ms", time_ms), ("mix", 1.0), ("feedback", 0.0)],
            sample_rate,
        );
        let expected = (time_ms * sample_rate as f32 / 1000.0).round() as usize;
        let response = impulse_response(&mut delay, expected + 100);
        for (index, value) in response.iter().enumerate() {
            let want = if index == expected { 1.0 } else { 0.0 };
            assert_eq!(*value, want, "time {time_ms} index {index}");
        }
    }
}

#[test]
fn delay_buffer_wraps_correctly_after_more_than_a_buffer_of_processing() {
    let sample_rate = 8_000;
    let capacity = (MAX_DELAY_SECONDS * sample_rate as f32) as usize;
    let delay_samples = 2_345;
    let time_ms = delay_samples as f32 * 1000.0 / sample_rate as f32;
    let mut delay = effect(
        EffectKind::Delay,
        &[("time_ms", time_ms), ("mix", 1.0), ("feedback", 0.0)],
        sample_rate,
    );
    let total = capacity * 3 + 777;
    let inputs: Vec<(f32, f32)> = (0..total).map(signal).collect();
    for (n, &(l, r)) in inputs.iter().enumerate() {
        let out = delay.process(l, r);
        let want = if n >= delay_samples {
            inputs[n - delay_samples]
        } else {
            (0.0, 0.0)
        };
        assert_eq!(out, want, "frame {n}");
    }

    // The maximum delay time also wraps exactly.
    let mut longest = effect(
        EffectKind::Delay,
        &[("time_ms", 4_000.0), ("mix", 1.0), ("feedback", 0.0)],
        sample_rate,
    );
    for n in 0..capacity * 2 + 10 {
        let (l, r) = signal(n);
        let out = longest.process(l, r);
        if n >= capacity {
            assert_eq!(out, signal(n - capacity), "frame {n}");
        }
    }
}

#[test]
fn delay_feedback_at_maximum_stays_finite_and_bounded() {
    for ping_pong in [0.0, 1.0] {
        let mut delay = effect(
            EffectKind::Delay,
            &[
                ("time_ms", 1.0),
                ("feedback", 0.95),
                ("mix", 1.0),
                ("damping", 20_000.0),
                ("ping_pong", ping_pong),
            ],
            48_000,
        );
        let mut peak = 0.0f32;
        for n in 0..48_000 * 10 {
            let (l, r) = delay.process(1.0, if n % 2 == 0 { 1.0 } else { -1.0 });
            assert!(l.is_finite() && r.is_finite());
            peak = peak.max(l.abs()).max(r.abs());
        }
        // Geometric series bound for |input| <= 1 and loop gain 0.95.
        assert!(peak <= 20.5, "peak {peak}");
    }

    // An impulse with maximum feedback decays.
    let mut delay = effect(
        EffectKind::Delay,
        &[("time_ms", 10.0), ("feedback", 0.95), ("mix", 1.0)],
        8_000,
    );
    let response = impulse_response(&mut delay, 8_000 * 10);
    let late = response[8_000 * 9..]
        .iter()
        .fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(late < 0.01, "late peak {late}");
}

#[test]
fn tempo_sync_recalculates_after_set_tempo() {
    let sample_rate = 8_000;
    // 1/4 note at 120 BPM = 0.5 s.
    let mut delay = effect(
        EffectKind::Delay,
        &[("sync", 1.0), ("mix", 1.0), ("feedback", 0.0)],
        sample_rate,
    );
    let response = impulse_response(&mut delay, 5_000);
    assert_eq!(response[4_000], 1.0);
    assert_eq!(response.iter().filter(|v| **v != 0.0).count(), 1);

    delay.set_tempo(60.0);
    for _ in 0..sample_rate {
        delay.process(0.0, 0.0);
    }
    let response = impulse_response(&mut delay, 9_000);
    assert_eq!(response[8_000], 1.0);
    assert_eq!(response.iter().filter(|v| **v != 0.0).count(), 1);

    // Dotted 1/8 at 100 BPM = 0.45 s, and switching sync off returns to time_ms.
    assert!(delay.set_param(param(EffectKind::Delay, "sync"), 3.0));
    delay.set_tempo(100.0);
    for _ in 0..sample_rate {
        delay.process(0.0, 0.0);
    }
    let response = impulse_response(&mut delay, 4_000);
    assert_eq!(response[3_600], 1.0);

    assert!(delay.set_param(param(EffectKind::Delay, "sync"), 0.0));
    assert!(delay.set_param(param(EffectKind::Delay, "time_ms"), 100.0));
    for _ in 0..sample_rate {
        delay.process(0.0, 0.0);
    }
    let response = impulse_response(&mut delay, 1_000);
    assert_eq!(response[800], 1.0);
}

#[test]
fn delay_time_change_glides_without_clicks() {
    let sample_rate = 48_000;
    let mut delay = effect(
        EffectKind::Delay,
        &[("time_ms", 100.0), ("mix", 1.0), ("feedback", 0.0)],
        sample_rate,
    );
    let tone = |n: usize| (n as f32 * 2.0 * std::f32::consts::PI * 220.0 / 48_000.0).sin();
    let mut previous = 0.0;
    for n in 0..sample_rate as usize {
        if n == 10_000 {
            assert!(delay.set_param(param(EffectKind::Delay, "time_ms"), 120.0));
        }
        let (l, _) = delay.process(tone(n), 0.0);
        if n > 5_000 {
            assert!((l - previous).abs() < 0.1, "step at {n}: {previous} -> {l}");
        }
        previous = l;
    }
}

#[test]
fn bypass_converges_to_dry_within_crossfade_without_large_steps() {
    let sample_rate = 48_000;
    let crossfade = sample_rate as usize / 100;
    let mut sat = effect(
        EffectKind::Saturation,
        &[("mode", 1.0), ("drive", 36.0)],
        sample_rate,
    );
    let input = 0.5;
    let mut previous = 0.0;
    for _ in 0..1_000 {
        previous = sat.process(input, input).0;
    }
    assert_eq!(previous, 1.0);

    sat.set_bypassed(true);
    assert!(sat.is_bypassed());
    let mut max_step = 0.0f32;
    for _ in 0..crossfade + 2 {
        let out = sat.process(input, input).0;
        max_step = max_step.max((out - previous).abs());
        previous = out;
    }
    assert!(max_step < 0.01, "bypass step {max_step}");
    assert_eq!(sat.process(input, -input), (input, -input));

    sat.set_bypassed(false);
    assert!(!sat.is_bypassed());
    max_step = 0.0;
    for _ in 0..crossfade + 2 {
        let out = sat.process(input, input).0;
        max_step = max_step.max((out - previous).abs());
        previous = out;
    }
    assert!(max_step < 0.01, "unbypass step {max_step}");
    assert_eq!(previous, 1.0);
}

#[test]
fn bypass_keeps_delay_state() {
    let mut delay = effect(
        EffectKind::Delay,
        &[("time_ms", 100.0), ("mix", 1.0), ("feedback", 0.0)],
        8_000,
    );
    delay.process(1.0, 1.0);
    delay.set_bypassed(true);
    for _ in 0..200 {
        delay.process(0.0, 0.0);
    }
    delay.set_bypassed(false);
    // The pending echo survives the bypass because state is frozen, not cleared.
    let heard = (0..2_000).any(|_| delay.process(0.0, 0.0).0.abs() > 0.5);
    assert!(heard);

    let config = config(EffectKind::Delay, &[("mix", 1.0)]);
    let mut bypassed_config = config.clone();
    bypassed_config.bypassed = true;
    let mut effect = Effect::new(&bypassed_config, 8_000, 120.0).unwrap();
    assert!(effect.is_bypassed());
    assert_eq!(effect.process(0.25, -0.5), (0.25, -0.5));
}

#[test]
fn reset_clears_delay_lines_and_reverb_tails() {
    for kind in [EffectKind::Delay, EffectKind::Reverb] {
        let mut fx = effect(kind, &[("mix", 1.0)], 8_000);
        for n in 0..2_000 {
            let (l, r) = signal(n);
            fx.process(l, r);
        }
        fx.reset();
        for _ in 0..40_000 {
            assert_eq!(fx.process(0.0, 0.0), (0.0, 0.0), "{kind:?}");
        }
    }
}

#[test]
fn reverb_tail_decays() {
    let sample_rate = 44_100;
    for (size, decay, damping) in [(0.5, 0.5, 0.5), (1.0, 1.0, 0.0), (0.0, 1.0, 1.0)] {
        let mut reverb = effect(
            EffectKind::Reverb,
            &[
                ("size", size),
                ("decay", decay),
                ("damping", damping),
                ("mix", 1.0),
            ],
            sample_rate,
        );
        let mut early = 0.0f64;
        let mut late = 0.0f64;
        for n in 0..sample_rate as usize * 6 {
            let input = if n == 0 { 1.0 } else { 0.0 };
            let (l, r) = reverb.process(input, input);
            assert!(l.is_finite() && r.is_finite());
            let energy = f64::from(l * l + r * r);
            let seconds = n as f32 / sample_rate as f32;
            if (0.05..0.5).contains(&seconds) {
                early += energy;
            } else if (5.5..6.0).contains(&seconds) {
                late += energy;
            }
        }
        assert!(early > 0.0, "reverb produced no tail");
        assert!(
            late < early * 0.1,
            "size {size} decay {decay}: {late} vs {early}"
        );
    }
}

#[test]
fn reverb_predelay_shifts_onset() {
    let sample_rate = 8_000;
    let mut reverb = effect(
        EffectKind::Reverb,
        &[("predelay_ms", 250.0), ("mix", 1.0)],
        sample_rate,
    );
    let response = impulse_response(&mut reverb, 4_000);
    let onset = response.iter().position(|v| *v != 0.0).unwrap();
    assert!(onset >= 2_000, "onset {onset}");
}

#[test]
fn rack_order_is_stable_and_deterministic() {
    let sat = config(EffectKind::Saturation, &[("drive", 24.0), ("mode", 1.0)]);
    let delay = config(
        EffectKind::Delay,
        &[("time_ms", 5.0), ("feedback", 0.6), ("mix", 0.5)],
    );
    let run = |configs: &[EffectConfig]| -> Vec<(f32, f32)> {
        let mut rack = EffectRack::new(configs, MAX_TRACK_INSERTS, 8_000, 120.0).unwrap();
        (0..4_000)
            .map(|n| {
                let (l, r) = signal(n);
                rack.process(l, r)
            })
            .collect()
    };
    let forward = run(&[sat.clone(), delay.clone()]);
    let reverse = run(&[delay.clone(), sat.clone()]);
    assert_ne!(forward, reverse);
    assert_eq!(forward, run(&[sat.clone(), delay.clone()]));
    assert_eq!(reverse, run(&[delay.clone(), sat.clone()]));

    let rack = EffectRack::new(&[sat, delay], MAX_TRACK_INSERTS, 8_000, 120.0).unwrap();
    assert_eq!(rack.len(), 2);
    assert!(!rack.is_empty());
    assert_eq!(rack.kind(EffectSlotId(0)), Some(EffectKind::Saturation));
    assert_eq!(rack.kind(EffectSlotId(1)), Some(EffectKind::Delay));
    assert_eq!(rack.kind(EffectSlotId(2)), None);
    assert!(EffectRack::default().is_empty());
}

#[test]
fn rack_routes_parameters_to_slots() {
    let mut rack = EffectRack::new(
        &[
            EffectConfig::new(EffectKind::Saturation),
            EffectConfig::new(EffectKind::Delay),
        ],
        MAX_MASTER_INSERTS,
        48_000,
        120.0,
    )
    .unwrap();
    let feedback = param(EffectKind::Delay, "feedback");
    assert_eq!(rack.param(EffectSlotId(1), feedback), Some(0.35));
    assert!(rack.set_param(EffectSlotId(1), feedback, 5.0));
    assert_eq!(rack.param(EffectSlotId(1), feedback), Some(0.95));
    assert!(!rack.set_param(EffectSlotId(1), feedback, f32::NAN));
    assert!(!rack.set_param(EffectSlotId(1), EffectParamId(42), 0.5));
    assert!(!rack.set_param(EffectSlotId(3), feedback, 0.5));
    assert_eq!(rack.param(EffectSlotId(3), feedback), None);
    assert!(rack.set_bypassed(EffectSlotId(0), true));
    assert!(!rack.set_bypassed(EffectSlotId(9), true));

    // Discrete values snap to whole steps.
    let sync = param(EffectKind::Delay, "sync");
    assert!(rack.set_param(EffectSlotId(1), sync, 2.4));
    assert_eq!(rack.param(EffectSlotId(1), sync), Some(2.0));
    rack.set_tempo(90.0);
    rack.reset();

    let too_many = vec![EffectConfig::new(EffectKind::Saturation); 3];
    assert!(EffectRack::new(&too_many, 2, 48_000, 120.0).is_err());
}

#[test]
fn processing_is_bit_identical_across_chunkings() {
    let configs = vec![
        config(
            EffectKind::Saturation,
            &[("drive", 12.0), ("tone", 3_000.0)],
        ),
        config(EffectKind::Delay, &[("time_ms", 7.3), ("feedback", 0.7)]),
        config(EffectKind::Reverb, &[("mix", 0.4)]),
    ];
    let render = |chunk: usize| -> Vec<(f32, f32)> {
        let mut rack = EffectRack::new(&configs, MAX_TRACK_INSERTS, 22_050, 120.0).unwrap();
        let mut out = Vec::new();
        let total = 6_000;
        let mut start = 0;
        while start < total {
            let end = (start + chunk).min(total);
            // Parameter changes are applied at chunk-independent frame positions.
            for n in start..end {
                if n == 1_000 {
                    rack.set_param(
                        EffectSlotId(0),
                        param(EffectKind::Saturation, "drive"),
                        30.0,
                    );
                }
                if n == 2_500 {
                    rack.set_param(EffectSlotId(1), param(EffectKind::Delay, "time_ms"), 20.0);
                    rack.set_tempo(140.0);
                }
                if n == 4_000 {
                    rack.set_bypassed(EffectSlotId(2), true);
                }
                let (l, r) = signal(n);
                out.push(rack.process(l, r));
            }
            start = end;
        }
        out
    };
    let reference = render(1);
    for chunk in [7, 64, 256, 6_000] {
        let other = render(chunk);
        assert!(
            reference
                .iter()
                .zip(&other)
                .all(|(a, b)| a.0.to_bits() == b.0.to_bits() && a.1.to_bits() == b.1.to_bits()),
            "chunk {chunk}"
        );
    }
}

#[test]
fn validation_rejects_bad_configurations() {
    let mut unknown = EffectConfig::new(EffectKind::Delay);
    unknown.params.insert("drive".into(), 1.0);
    assert!(unknown.validate().is_err());
    assert!(Effect::new(&unknown, 48_000, 120.0).is_err());

    let mut out_of_range = EffectConfig::new(EffectKind::Delay);
    out_of_range.params.insert("feedback".into(), 1.2);
    assert!(out_of_range.validate().is_err());
    let mut non_finite = EffectConfig::new(EffectKind::Reverb);
    non_finite.params.insert("mix".into(), f32::NAN);
    assert!(non_finite.validate().is_err());

    let mut setter = EffectConfig::new(EffectKind::Saturation);
    assert!(setter.set_param_value(EffectParamId(1), 99.0).is_err());
    assert!(setter.set_param_value(EffectParamId(9), 1.0).is_err());
    assert!(setter.set_param_value(EffectParamId(1), 12.0).is_ok());
    assert_eq!(setter.param_value(EffectParamId(1)), 12.0);
    assert_eq!(setter.param_value(EffectParamId(3)), 1.0);
    assert_eq!(setter.params.get("drive"), Some(&12.0));

    let insert = EffectConfig::new(EffectKind::Saturation);
    assert!(validate_track_inserts(&vec![insert.clone(); MAX_TRACK_INSERTS]).is_ok());
    assert!(validate_track_inserts(&vec![insert.clone(); MAX_TRACK_INSERTS + 1]).is_err());
    assert!(validate_track_inserts(&[unknown.clone()]).is_err());

    let bus = SendBusConfig {
        effects: vec![insert.clone(); MAX_SEND_EFFECTS],
        return_gain: 1.0,
    };
    let valid = EffectsConfig {
        send_buses: vec![bus.clone(); MAX_SEND_BUSES],
        master: vec![insert.clone(); MAX_MASTER_INSERTS],
    };
    assert!(valid.validate().is_ok());

    let mut too_many_buses = valid.clone();
    too_many_buses.send_buses.push(bus.clone());
    assert!(too_many_buses.validate().is_err());

    let mut crowded_bus = valid.clone();
    crowded_bus.send_buses[1].effects.push(insert.clone());
    assert!(crowded_bus.validate().is_err());

    let mut crowded_master = valid.clone();
    crowded_master.master.push(insert.clone());
    assert!(crowded_master.validate().is_err());

    let mut loud_return = valid.clone();
    loud_return.send_buses[0].return_gain = 2.5;
    assert!(loud_return.validate().is_err());

    let mut bad_master = valid;
    bad_master.master[0] = unknown;
    assert!(bad_master.validate().is_err());
}

#[test]
fn memory_budget_rejects_oversized_configurations() {
    let delay = EffectConfig::new(EffectKind::Delay);
    let reverb = EffectConfig::new(EffectKind::Reverb);
    let sat = EffectConfig::new(EffectKind::Saturation);
    assert_eq!(sat.required_memory_bytes(48_000), 0);
    let delay_bytes = delay.required_memory_bytes(48_000);
    assert!(delay_bytes >= 2 * 4 * 48_000 * 4);
    assert!(reverb.required_memory_bytes(48_000) > 0);
    assert!(reverb.required_memory_bytes(96_000) > reverb.required_memory_bytes(48_000));

    let config = EffectsConfig {
        send_buses: vec![SendBusConfig {
            effects: vec![delay.clone(), reverb.clone()],
            return_gain: 1.0,
        }],
        master: vec![delay.clone()],
    };
    let total = config.required_memory_bytes(48_000);
    assert_eq!(
        total,
        delay_bytes * 2 + reverb.required_memory_bytes(48_000)
    );
    assert!(check_effect_memory(total, DEFAULT_EFFECT_MEMORY_BUDGET).is_ok());
    assert!(check_effect_memory(total, total).is_ok());
    assert!(check_effect_memory(total, total - 1).is_err());
    assert!(check_effect_memory(
        DEFAULT_EFFECT_MEMORY_BUDGET + 1,
        DEFAULT_EFFECT_MEMORY_BUDGET
    )
    .is_err());
}

#[test]
fn effects_config_round_trips_through_serde_with_snake_case_names() {
    let config = EffectsConfig {
        send_buses: vec![
            SendBusConfig {
                effects: vec![config(
                    EffectKind::Delay,
                    &[("time_ms", 250.0), ("ping_pong", 1.0)],
                )],
                return_gain: 0.8,
            },
            SendBusConfig {
                effects: vec![config(EffectKind::Reverb, &[("predelay_ms", 20.0)])],
                return_gain: 1.0,
            },
        ],
        master: vec![{
            let mut sat = EffectConfig::new(EffectKind::Saturation);
            sat.bypassed = true;
            sat
        }],
    };
    let json = serde_json::to_string(&config).unwrap();
    assert!(json.contains("\"kind\":\"delay\""), "{json}");
    assert!(json.contains("\"time_ms\":250.0"), "{json}");
    assert!(json.contains("\"ping_pong\":1.0"), "{json}");
    assert!(
        json.contains("\"kind\":\"saturation\",\"bypassed\":true"),
        "{json}"
    );
    let parsed: EffectsConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed, config);
    assert!(parsed.validate().is_ok());

    assert!(EffectsConfig::default().is_empty());
    assert_eq!(
        serde_json::to_string(&EffectsConfig::default()).unwrap(),
        "{}"
    );
    let minimal: EffectsConfig =
        serde_json::from_str(r#"{"send_buses":[{"effects":[{"kind":"reverb"}]}]}"#).unwrap();
    assert_eq!(minimal.send_buses[0].return_gain, 1.0);
    assert!(minimal.send_buses[0].effects[0].params.is_empty());
    assert!(serde_json::from_str::<EffectsConfig>(r#"{"master":[{"kind":"chorus"}]}"#).is_err());
    assert!(
        serde_json::from_str::<EffectsConfig>(r#"{"master":[{"kind":"delay","wet":1}]}"#).is_err()
    );
}
