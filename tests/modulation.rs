//! Spec 13 acceptance tests: modulation matrix and macros.

use shelloop::modulation::runtime::lfo_value;
use shelloop::{
    apply_mod_command, parse_mod_command, parse_synth_parameter_command, CompiledModulation,
    CompiledSynthPatch, EngineCommand, LfoWave, ModDest, ModInputs, ModRoute, ModRuntime,
    ModSource, ModulationConfig, Oscillator, RealtimeSynth, SyncDivision, SynthParamId,
    SynthParamValue, SynthPatch, MAX_ROUTES, MOD_CONTROL_INTERVAL,
};

fn render(patch: SynthPatch, sample_rate: f32, frames: usize) -> Vec<f32> {
    let mut synth = RealtimeSynth::new_with_patch(sample_rate, patch, 4).unwrap();
    synth.handle(EngineCommand::NoteOn {
        channel: 0,
        note: 57,
        velocity: 0.9,
    });
    (0..frames).map(|_| synth.next_sample()).collect()
}

fn filtered_patch() -> SynthPatch {
    let mut patch = SynthPatch::legacy(Oscillator::Saw);
    patch.filter.mode = shelloop::FilterMode::LowPass;
    patch.filter.cutoff_hz = 1_500.0;
    patch.filter.resonance = 0.3;
    patch
}

fn with_route(mut patch: SynthPatch, route: ModRoute) -> SynthPatch {
    patch.modulation.add_route(route).unwrap();
    patch
}

#[test]
fn zero_modulation_renders_bit_identically_to_legacy() {
    let legacy = filtered_patch();
    let baseline = render(legacy, 48_000.0, 4_000);

    let mut disabled = legacy;
    let mut route = ModRoute::new(ModSource::Lfo1, ModDest::FilterCutoff, 1.0);
    route.enabled = false;
    disabled.modulation.add_route(route).unwrap();
    disabled
        .modulation
        .add_route(ModRoute::new(ModSource::Lfo2, ModDest::Tuning, 0.0))
        .unwrap();
    // Macros and LFO settings with no live route must not change the sound.
    disabled.modulation.macros = [0.7, 0.3];
    disabled.modulation.lfos[0].wave = LfoWave::Square;

    let candidate = render(disabled, 48_000.0, 4_000);
    assert_eq!(
        baseline.iter().map(|v| v.to_bits()).collect::<Vec<_>>(),
        candidate.iter().map(|v| v.to_bits()).collect::<Vec<_>>()
    );
}

#[test]
fn active_modulation_changes_the_sound_and_stays_bounded() {
    let base = filtered_patch();
    let baseline = render(base, 48_000.0, 6_000);
    for dest in [
        ModDest::Tuning,
        ModDest::PulseWidth,
        ModDest::FilterCutoff,
        ModDest::FilterResonance,
        ModDest::Amplitude,
    ] {
        let mut patch = base;
        patch.oscillator = Oscillator::Pulse;
        let reference = render(patch, 48_000.0, 6_000);
        let mut route = ModRoute::new(ModSource::Lfo1, dest, 0.8);
        route.smoothing_ms = 1.0;
        let mut modulated = with_route(patch, route);
        modulated.modulation.lfos[0].rate_hz = 20.0;
        let out = render(modulated, 48_000.0, 6_000);
        assert!(
            out.iter().all(|v| v.is_finite() && v.abs() <= 1.0),
            "{dest:?}"
        );
        assert_ne!(out, reference, "{dest:?} had no effect");
    }
    assert_ne!(baseline.len(), 0);
}

#[test]
fn lfo_rate_is_wall_clock_across_sample_rates_and_tempo_sync_follows_bpm() {
    for sample_rate in [44_100.0_f32, 48_000.0, 96_000.0] {
        let mut config = ModulationConfig::default();
        config.lfos[0].rate_hz = 5.0;
        config
            .add_route(ModRoute::new(ModSource::Lfo1, ModDest::Pan, 1.0))
            .unwrap();
        let compiled = CompiledModulation::compile(&config, sample_rate).unwrap();
        let mut runtime = ModRuntime::new(sample_rate, 120.0, compiled);
        runtime.note_on(60, [0.0; 2]);
        let frames = sample_rate as usize * 2;
        let mut rising = 0;
        let mut previous = 0.0;
        for _ in 0..frames {
            let value = runtime.next(&ModInputs::default()).pan;
            if previous < 0.0 && value >= 0.0 {
                rising += 1;
            }
            previous = value;
        }
        assert!(
            (9..=11).contains(&rising),
            "{sample_rate}: {rising} cycles in 2 s"
        );
    }

    let count = |bpm: f64, retempo: Option<f64>| {
        let mut config = ModulationConfig::default();
        config.lfos[0].sync = Some(SyncDivision::Quarter);
        config
            .add_route(ModRoute::new(ModSource::Lfo1, ModDest::Pan, 1.0))
            .unwrap();
        let compiled = CompiledModulation::compile(&config, 48_000.0).unwrap();
        let mut runtime = ModRuntime::new(48_000.0, bpm, compiled);
        if let Some(new) = retempo {
            runtime.set_tempo(new);
        }
        runtime.note_on(60, [0.0; 2]);
        let mut rising = 0;
        let mut previous = 0.0;
        for _ in 0..48_000 * 4 {
            let value = runtime.next(&ModInputs::default()).pan;
            if previous < 0.0 && value >= 0.0 {
                rising += 1;
            }
            previous = value;
        }
        rising
    };
    // 1/4 note: 120 bpm = 2 Hz (8 cycles in 4 s), 60 bpm = 1 Hz (4 cycles).
    assert!((7..=9).contains(&count(120.0, None)));
    assert!((3..=5).contains(&count(60.0, None)));
    assert!((7..=9).contains(&count(60.0, Some(120.0))));
}

#[test]
fn sample_and_hold_is_deterministic_per_seed_and_constant_within_a_cycle() {
    let sequence = |seed: u32| {
        (0..64)
            .map(|cycle| lfo_value(LfoWave::SampleHold, f64::from(cycle) + 0.25, seed))
            .collect::<Vec<_>>()
    };
    assert_eq!(sequence(7), sequence(7));
    assert_ne!(sequence(7), sequence(8));
    for cycle in 0..32 {
        let a = lfo_value(LfoWave::SampleHold, f64::from(cycle) + 0.1, 3);
        let b = lfo_value(LfoWave::SampleHold, f64::from(cycle) + 0.9, 3);
        assert_eq!(a, b);
        assert!((-1.0..1.0).contains(&a));
    }
    let values = sequence(7);
    assert!(values.iter().any(|v| *v > 0.3) && values.iter().any(|v| *v < -0.3));
}

#[test]
fn lfo_waves_have_expected_shape_and_range() {
    for wave in [
        LfoWave::Sine,
        LfoWave::Triangle,
        LfoWave::Saw,
        LfoWave::Square,
    ] {
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        for i in 0..1000 {
            let v = lfo_value(wave, f64::from(i) / 1000.0, 1);
            assert!(v.is_finite());
            min = min.min(v);
            max = max.max(v);
        }
        assert!(min <= -0.99 && max >= 0.99, "{wave:?}");
        assert!(min >= -1.0 && max <= 1.0, "{wave:?}");
    }
    assert_eq!(lfo_value(LfoWave::Saw, 0.0, 1), -1.0);
    assert_eq!(lfo_value(LfoWave::Square, 0.25, 1), 1.0);
    assert_eq!(lfo_value(LfoWave::Square, 0.75, 1), -1.0);
}

#[test]
fn routes_sum_clamp_and_stay_finite_at_extremes() {
    let mut config = ModulationConfig::default();
    for _ in 0..MAX_ROUTES {
        config
            .add_route(ModRoute::new(ModSource::Velocity, ModDest::Tuning, 1.0))
            .unwrap();
    }
    let compiled = CompiledModulation::compile(&config, 48_000.0).unwrap();
    let mut runtime = ModRuntime::new(48_000.0, 120.0, compiled);
    runtime.note_on(60, [0.0; 2]);
    let inputs = ModInputs {
        velocity: 1.0,
        amp_env: 1.0,
        filter_env: 1.0,
        mod_wheel: 1.0,
    };
    let offsets = runtime.next(&inputs);
    assert_eq!(
        offsets.tuning_semitones, 24.0,
        "8 routes x 12 st clamp to +-24"
    );

    // Two routes to one destination add in the defined order-independent sum.
    let mut config = ModulationConfig::default();
    config.macros = [0.5, 1.0];
    config
        .add_route(ModRoute::new(ModSource::Macro1, ModDest::Pan, 0.5))
        .unwrap();
    config
        .add_route(ModRoute::new(ModSource::Macro2, ModDest::Pan, -0.25))
        .unwrap();
    let mut runtime = ModRuntime::new(
        48_000.0,
        120.0,
        CompiledModulation::compile(&config, 48_000.0).unwrap(),
    );
    runtime.note_on(60, [0.0; 2]);
    assert!((runtime.next(&inputs).pan - 0.0).abs() < 1e-6);

    // Synth output remains finite at every extreme.
    let mut patch = filtered_patch();
    for dest in [
        ModDest::Tuning,
        ModDest::FilterCutoff,
        ModDest::FilterResonance,
        ModDest::Amplitude,
        ModDest::PulseWidth,
        ModDest::Pan,
        ModDest::EffectWet,
    ] {
        patch
            .modulation
            .add_route(ModRoute::new(ModSource::Lfo1, dest, 1.0))
            .ok();
    }
    patch.modulation.lfos[0].rate_hz = 50.0;
    for sample_rate in [8_000.0, 44_100.0, 192_000.0] {
        let out = render(patch, sample_rate, 8_000);
        assert!(out.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
    }
}

#[test]
fn retrigger_restarts_lfo_phase_and_free_run_joins_shared_phase() {
    let make = |retrigger: bool| {
        let mut config = ModulationConfig::default();
        config.lfos[0].rate_hz = 1.0;
        config.lfos[0].retrigger = retrigger;
        config.lfos[0].wave = LfoWave::Saw;
        config
            .add_route(ModRoute::new(ModSource::Lfo1, ModDest::Pan, 1.0))
            .unwrap();
        ModRuntime::new(
            48_000.0,
            120.0,
            CompiledModulation::compile(&config, 48_000.0).unwrap(),
        )
    };
    let mut retrig = make(true);
    retrig.note_on(60, [0.5, 0.5]);
    assert!((retrig.next(&ModInputs::default()).pan - -1.0).abs() < 1e-3);
    let mut free = make(false);
    free.note_on(60, [0.5, 0.5]);
    assert!(free.next(&ModInputs::default()).pan.abs() < 1e-3);
}

#[test]
fn legacy_patches_load_unchanged_and_modulation_round_trips() {
    let legacy = SynthPatch::legacy(Oscillator::Saw);
    let json = serde_json::to_value(legacy).unwrap();
    assert!(
        json.get("modulation").is_none(),
        "default modulation is not serialised"
    );
    let back: SynthPatch = serde_json::from_value(json).unwrap();
    assert_eq!(back, legacy);

    let mut patch = filtered_patch();
    patch.modulation.macros = [0.25, 0.75];
    patch.modulation.lfos[1].wave = LfoWave::SampleHold;
    patch.modulation.lfos[1].sync = Some(SyncDivision::EighthTriplet);
    let mut route = ModRoute::new(ModSource::Macro1, ModDest::FilterCutoff, -0.5);
    route.smoothing_ms = 20.0;
    patch.modulation.add_route(route).unwrap();
    let text = serde_json::to_string(&patch).unwrap();
    let back: SynthPatch = serde_json::from_str(&text).unwrap();
    assert_eq!(back, patch);
    assert!(text.contains("1/8t") && text.contains("sample_hold"));
}

#[test]
fn invalid_modulation_is_rejected_with_clear_errors() {
    let parse =
        |json: &str| serde_json::from_str::<ModulationConfig>(json).map_err(|e| e.to_string());
    assert!(parse("{}").is_ok());
    assert!(parse(r#"{"version":2}"#).unwrap_err().contains("version"));
    assert!(parse(r#"{"routes":[{"source":"lfo1","dest":"pan","amount":1.5}]}"#).is_err());
    assert!(parse(r#"{"routes":[{"source":"nope","dest":"pan","amount":1.0}]}"#).is_err());
    assert!(
        parse(r#"{"routes":[{"source":"lfo1","dest":"pan","amount":0.5,"extra":1}]}"#).is_err()
    );
    assert!(parse(r#"{"macros":[0.1,0.2,0.3]}"#).is_err());
    assert!(parse(r#"{"macros":[1.5]}"#).is_err());
    assert!(parse(r#"{"lfos":[{"rate_hz":0.0}]}"#).is_err());
    let nine = format!(
        r#"{{"routes":[{}]}}"#,
        [r#"{"source":"lfo1","dest":"pan","amount":0.1}"#; 9].join(",")
    );
    assert!(parse(&nine).unwrap_err().contains("at most 8"));

    let mut config = ModulationConfig::default();
    for _ in 0..MAX_ROUTES {
        config
            .add_route(ModRoute::new(ModSource::Lfo1, ModDest::Pan, 0.1))
            .unwrap();
    }
    assert!(config
        .add_route(ModRoute::new(ModSource::Lfo1, ModDest::Pan, 0.1))
        .is_err());
    let mut nan = ModRoute::new(ModSource::Lfo1, ModDest::Pan, f32::NAN);
    assert!(ModulationConfig::default().add_route(nan).is_err());
    nan.amount = 0.0;
    nan.smoothing_ms = f32::INFINITY;
    assert!(ModulationConfig::default().add_route(nan).is_err());
}

#[test]
fn mod_commands_edit_routes_atomically() {
    let mut config = ModulationConfig::default();
    let run = |config: &mut ModulationConfig, line: &str| {
        apply_mod_command(config, parse_mod_command(line)?)
    };
    run(&mut config, "mod add lfo1 filter_cutoff 0.5 25").unwrap();
    run(&mut config, "mod add macro1 pan -0.25").unwrap();
    assert_eq!(config.routes().len(), 2);
    assert_eq!(config.routes()[0].smoothing_ms, 25.0);
    run(&mut config, "mod off 1").unwrap();
    assert!(!config.routes()[0].enabled);
    run(&mut config, "mod set 2 velocity amplitude 1").unwrap();
    assert_eq!(config.routes()[1].source, ModSource::Velocity);
    let before = config;
    assert!(run(&mut config, "mod add lfo1 pan 9").is_err());
    assert!(run(&mut config, "mod add bogus pan 0.1").is_err());
    assert!(run(&mut config, "mod remove 5").is_err());
    assert!(run(&mut config, "mod frobnicate").is_err());
    assert_eq!(config, before, "failed commands leave the config unchanged");
    run(&mut config, "mod remove 1").unwrap();
    assert_eq!(config.routes().len(), 1);
    assert!(run(&mut config, "mod list").unwrap().contains("Velocity"));
    run(&mut config, "mod clear").unwrap();
    assert!(config.routes().is_empty());
}

#[test]
fn macro_and_lfo_synth_parameters_use_stable_ids_and_commands() {
    let sample_rate = 48_000.0;
    let patch = filtered_patch();
    let (id, value) = parse_synth_parameter_command("synth macro1 0.5").unwrap();
    assert_eq!(id, SynthParamId::Macro1);
    let patch = patch.with_parameter(id, value, sample_rate).unwrap();
    assert_eq!(patch.modulation.macros[0], 0.5);
    let (id, value) = parse_synth_parameter_command("synth lfo2_wave triangle").unwrap();
    assert_eq!(value, SynthParamValue::LfoWave(LfoWave::Triangle));
    let patch = patch.with_parameter(id, value, sample_rate).unwrap();
    assert_eq!(patch.modulation.lfos[1].wave, LfoWave::Triangle);
    let (id, value) = parse_synth_parameter_command("synth lfo1_sync 1/8").unwrap();
    let patch = patch.with_parameter(id, value, sample_rate).unwrap();
    assert_eq!(patch.modulation.lfos[0].sync, Some(SyncDivision::Eighth));
    let (id, value) = parse_synth_parameter_command("synth lfo1_sync off").unwrap();
    let patch = patch.with_parameter(id, value, sample_rate).unwrap();
    assert_eq!(patch.modulation.lfos[0].sync, None);
    assert!(patch
        .with_parameter(
            SynthParamId::Macro2,
            SynthParamValue::Number(1.5),
            sample_rate
        )
        .is_err());
    assert!(patch
        .with_parameter(
            SynthParamId::Lfo1Wave,
            SynthParamValue::Number(9.0),
            sample_rate
        )
        .is_err());
    assert!(patch
        .with_parameter(
            SynthParamId::Lfo1Rate,
            SynthParamValue::Number(0.0),
            sample_rate
        )
        .is_err());
    assert!(CompiledSynthPatch::new(sample_rate, patch).is_ok());
    // Stable snake_case identifiers.
    for name in [
        "macro1",
        "macro2",
        "lfo1_rate",
        "lfo2_rate",
        "lfo1_wave",
        "lfo2_wave",
        "lfo1_sync",
        "lfo2_sync",
    ] {
        assert!(
            shelloop::params::parse_synth_param(name).is_some(),
            "{name}"
        );
    }
}

#[test]
fn macro_value_routes_to_cutoff_and_live_patch_updates_keep_notes_playing() {
    let sample_rate = 48_000.0;
    let mut patch = filtered_patch();
    patch
        .modulation
        .add_route(ModRoute::new(
            ModSource::Macro1,
            ModDest::FilterCutoff,
            -1.0,
        ))
        .unwrap();
    let mut synth = RealtimeSynth::new_with_patch(sample_rate, patch, 2).unwrap();
    synth.handle(EngineCommand::NoteOn {
        channel: 0,
        note: 69,
        velocity: 1.0,
    });
    let energy =
        |synth: &mut RealtimeSynth| (0..6_000).map(|_| synth.next_sample().powi(2)).sum::<f32>();
    let open = energy(&mut synth);
    let closed_patch = patch
        .with_parameter(
            SynthParamId::Macro1,
            SynthParamValue::Number(1.0),
            sample_rate,
        )
        .unwrap();
    assert!(synth.apply_patch(CompiledSynthPatch::new(sample_rate, closed_patch).unwrap()));
    energy(&mut synth); // glide
    let closed = energy(&mut synth);
    assert!(
        closed < open * 0.5,
        "closing the macro should darken the sound ({closed} vs {open})"
    );
    assert_eq!(synth.active_voice_count(), 1);
}

#[test]
fn mod_wheel_source_follows_engine_command() {
    let mut patch = filtered_patch();
    patch
        .modulation
        .add_route(ModRoute::new(ModSource::ModWheel, ModDest::Amplitude, -1.0))
        .unwrap();
    let mut synth = RealtimeSynth::new_with_patch(48_000.0, patch, 2).unwrap();
    synth.handle(EngineCommand::NoteOn {
        channel: 0,
        note: 60,
        velocity: 1.0,
    });
    let loud: f32 = (0..4_000).map(|_| synth.next_sample().abs()).sum();
    synth.handle(EngineCommand::ModWheel { value: 1.0 });
    for _ in 0..200 {
        synth.next_sample();
    }
    let silent: f32 = (0..4_000).map(|_| synth.next_sample().abs()).sum();
    assert!(loud > 1.0);
    assert!(
        silent < 1e-3,
        "mod wheel at max with -1 amplitude depth mutes: {silent}"
    );
}

#[test]
fn pan_and_effect_wet_destinations_are_reported_to_the_track_mixer() {
    let mut patch = filtered_patch();
    patch
        .modulation
        .add_route(ModRoute::new(ModSource::Velocity, ModDest::Pan, -0.5))
        .unwrap();
    patch
        .modulation
        .add_route(ModRoute::new(ModSource::Velocity, ModDest::EffectWet, 0.25))
        .unwrap();
    let mut synth = RealtimeSynth::new_with_patch(48_000.0, patch, 2).unwrap();
    assert_eq!(synth.modulation_pan_and_wet(), (0.0, 0.0));
    synth.handle(EngineCommand::NoteOn {
        channel: 0,
        note: 60,
        velocity: 1.0,
    });
    for _ in 0..64 {
        synth.next_sample();
    }
    let (pan, wet) = synth.modulation_pan_and_wet();
    assert!(
        (pan + 0.5).abs() < 1e-3 && (wet - 0.25).abs() < 1e-3,
        "{pan} {wet}"
    );
}

#[test]
fn control_interval_is_a_documented_constant() {
    assert_eq!(MOD_CONTROL_INTERVAL, 16);
}

#[test]
fn inspector_ranges_preserve_negative_unipolar_route_direction() {
    let patch = SynthPatch::legacy(Oscillator::Saw);
    let route = ModRoute::new(ModSource::Macro1, ModDest::Amplitude, -0.5);
    assert_eq!(shelloop::route_range_text(&patch, &route), "x0.50..x1.00");
}
