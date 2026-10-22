//! Spec 14 acceptance tests: sample shaping, slices and their control path.

use crossbeam_channel::bounded;
use shelloop::params::sample_param_descriptor;
use shelloop::{
    apply_audio_message, apply_sample_command, detect_slices, equal_slices, parse_sample_command,
    CompiledSamplePlayback, CompiledShaping, EngineCommand, MultiTrackEngine, MultiTrackProject,
    Pattern, PatternStep, RealtimeSampler, SampleAsset, SampleAssetBank, SampleAssetId,
    SampleContext, SampleEnvelope, SampleFilterMode, SampleLockOverrides, SampleMode,
    SampleParamId, SampleSettings, SampleShaping, SessionController, SliceDef, SliceSelect,
    SliceSettings, SliceTable, StereoFrame, TrackDefinition, TrackId, TrackKind, MAX_SLICES,
};
use std::path::PathBuf;

fn asset(frames: Vec<StereoFrame>, rate: u32) -> SampleAsset {
    SampleAsset {
        id: SampleAssetId(0),
        source_path: PathBuf::from("fixture.wav"),
        source_sample_rate: rate,
        channels: 1,
        frames: frames.into(),
    }
}

/// Frame N holds value N so output reveals which source frames were read.
fn ramp(frames: usize) -> SampleAsset {
    asset(
        (0..frames)
            .map(|i| StereoFrame::new(i as f32, i as f32))
            .collect(),
        48_000,
    )
}

fn settings(mode: SampleMode) -> SampleSettings {
    let mut s = SampleSettings::new("fixture.wav", mode);
    s.release_secs = 0.0;
    s
}

fn sampler(asset: &SampleAsset, settings: &SampleSettings) -> RealtimeSampler {
    RealtimeSampler::new(CompiledSamplePlayback::new(asset, settings, 48_000).unwrap())
}

fn note_on(s: &mut RealtimeSampler, note: u8) {
    s.handle(EngineCommand::NoteOn {
        channel: 0,
        note,
        velocity: 1.0,
    });
}

fn render(s: &mut RealtimeSampler, frames: usize) -> Vec<f32> {
    (0..frames).map(|_| s.next_stereo_frame().0).collect()
}

fn sliced(select: SliceSelect) -> SampleSettings {
    let mut s = settings(SampleMode::OneShot);
    s.slices.regions = equal_slices(4).unwrap();
    s.slices.select = select;
    s
}

#[test]
fn legacy_settings_serialise_without_new_blocks_and_old_files_load() {
    let s = SampleSettings::new("kick.wav", SampleMode::OneShot);
    let json = serde_json::to_value(&s).unwrap();
    assert!(json.get("sample_shaping").is_none() && json.get("slices").is_none());
    let back: SampleSettings = serde_json::from_value(json).unwrap();
    assert_eq!(back, s);
    let old: SampleSettings =
        serde_json::from_str(r#"{"path":"a.wav","mode":"one_shot","start":0.1}"#).unwrap();
    assert!(old.sample_shaping.is_default() && old.slices.is_default());
}

#[test]
fn shaping_and_slices_round_trip_and_validate() {
    let mut s = settings(SampleMode::Gate);
    s.sample_shaping.filter_mode = SampleFilterMode::HighPass;
    s.sample_shaping.filter_cutoff = 800.0;
    s.sample_shaping.drive_db = 6.0;
    s.sample_shaping.envelope = Some(SampleEnvelope {
        attack_secs: 0.01,
        decay_secs: 0.1,
        sustain: 0.5,
        release_secs: 0.2,
    });
    s.sample_shaping.transient_attack = 0.5;
    s.slices = SliceSettings {
        regions: equal_slices(3).unwrap(),
        select: SliceSelect::Note,
        base_note: 40,
        seed: 9,
        ..SliceSettings::default()
    };
    let text = serde_json::to_string(&s).unwrap();
    assert_eq!(serde_json::from_str::<SampleSettings>(&text).unwrap(), s);

    let bad = |f: &dyn Fn(&mut SampleShaping)| {
        let mut shaping = SampleShaping::default();
        f(&mut shaping);
        shaping.validate().is_err()
    };
    assert!(bad(&|s| s.filter_cutoff = 5.0));
    assert!(bad(&|s| s.filter_resonance = 1.0));
    assert!(bad(&|s| s.drive_db = 40.0));
    assert!(bad(&|s| s.transient_attack = f32::NAN));
    assert!(bad(&|s| s.version = 9));
    assert!(bad(&|s| s.envelope = Some(SampleEnvelope {
        attack_secs: -1.0,
        decay_secs: 0.0,
        sustain: 1.0,
        release_secs: 0.0
    })));
    assert!(serde_json::from_str::<SampleShaping>(r#"{"bogus":1}"#).is_err());
}

#[test]
fn slice_validation_rejects_empty_overlapping_and_excess_slices() {
    let mut slices = SliceSettings::default();
    let add = |s: &mut SliceSettings, a: f64, b: f64| s.add(SliceDef { start: a, end: b });
    assert_eq!(add(&mut slices, 0.0, 0.5), Ok(0));
    assert!(add(&mut slices, 0.5, 0.5).is_err(), "empty");
    assert!(add(&mut slices, 0.7, 0.6).is_err(), "reversed");
    assert!(add(&mut slices, 0.4, 0.6).is_err(), "overlap");
    assert!(add(&mut slices, f64::NAN, 0.6).is_err(), "nan");
    assert!(add(&mut slices, 0.5, 1.5).is_err(), "out of range");
    assert_eq!(
        add(&mut slices, 0.5, 1.0),
        Ok(1),
        "touching slices are allowed"
    );
    assert_eq!(
        slices.regions.len(),
        2,
        "failed adds leave the map unchanged"
    );
    assert!(slices.delete(5).is_err());
    slices.delete(0).unwrap();
    assert_eq!(slices.regions.len(), 1);

    assert_eq!(equal_slices(MAX_SLICES).unwrap().len(), MAX_SLICES);
    assert!(equal_slices(MAX_SLICES + 1).is_err() && equal_slices(0).is_err());
    let too_many = SliceSettings {
        regions: (0..=MAX_SLICES)
            .map(|i| SliceDef {
                start: i as f64 / 200.0,
                end: (i as f64 + 0.5) / 200.0,
            })
            .collect(),
        ..SliceSettings::default()
    };
    assert!(too_many.validate().is_err());
    let note_overflow = SliceSettings {
        regions: equal_slices(10).unwrap(),
        select: SliceSelect::Note,
        base_note: 125,
        ..SliceSettings::default()
    };
    assert!(note_overflow.validate().is_err());
}

#[test]
fn offline_detection_finds_transients_deterministically() {
    let rate = 48_000;
    let mut frames = vec![StereoFrame::new(0.0, 0.0); rate as usize];
    let hits = [0.0_f64, 0.25, 0.5, 0.75];
    for hit in hits {
        let start = (hit * rate as f64) as usize + 100;
        for i in 0..2_000 {
            let envelope = (-(i as f32) / 400.0).exp();
            let v = envelope * (i as f32 * 0.3).sin();
            frames[start + i] = StereoFrame::new(v, v);
        }
    }
    let a = detect_slices(&frames, rate, 16, 0.6).unwrap();
    let b = detect_slices(&frames, rate, 16, 0.6).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.len(), 4, "{a:?}");
    for (slice, hit) in a.iter().zip(hits) {
        assert!(
            (slice.start - hit).abs() < 0.02 || slice.start == 0.0,
            "{slice:?} vs {hit}"
        );
    }
    assert_eq!(a.last().unwrap().end, 1.0);
    let capped = detect_slices(&frames, rate, 2, 0.6).unwrap();
    assert!(capped.len() <= 2);
    let silent = vec![StereoFrame::new(0.0, 0.0); 1000];
    assert_eq!(detect_slices(&silent, rate, 8, 0.5).unwrap().len(), 1);
    assert!(detect_slices(&[], rate, 8, 0.5).is_err());
    assert!(detect_slices(&frames, rate, 8, 2.0).is_err());
    let validated = SliceSettings {
        regions: a,
        ..SliceSettings::default()
    };
    assert!(validated.validate().is_ok());
}

#[test]
fn note_mapped_slices_play_exactly_their_region() {
    let a = ramp(1000);
    let mut s = sampler(&a, &sliced(SliceSelect::Note));
    note_on(&mut s, 37);
    let out = render(&mut s, 300);
    assert_eq!(
        &out[..250],
        (250..500).map(|v| v as f32).collect::<Vec<_>>()
    );
    assert!(
        out[250..].iter().all(|v| *v == 0.0),
        "slice ends at its boundary"
    );
    assert_eq!(s.active_voice_count(), 0);

    note_on(&mut s, 36);
    assert_eq!(render(&mut s, 2), vec![0.0, 1.0]);
    s.handle(EngineCommand::Panic);

    // Notes outside the mapped range are silent rather than clamped.
    note_on(&mut s, 35);
    note_on(&mut s, 40);
    assert_eq!(s.active_voice_count(), 0);
}

#[test]
fn slices_respect_reverse_loop_gate_and_root_note() {
    let a = ramp(1000);
    let mut reverse = sliced(SliceSelect::Note);
    reverse.reverse = true;
    let mut s = sampler(&a, &reverse);
    note_on(&mut s, 37);
    let out = render(&mut s, 250);
    assert_eq!(out[0], 499.0);
    assert_eq!(out[249], 250.0);
    assert_eq!(s.active_voice_count(), 0);

    let mut looped = sliced(SliceSelect::Note);
    looped.mode = SampleMode::Loop;
    let mut s = sampler(&a, &looped);
    note_on(&mut s, 38);
    let out = render(&mut s, 2_000);
    assert!(
        out.iter().all(|v| (500.0..750.0).contains(v)),
        "loop stays inside the slice"
    );
    assert_eq!(s.active_voice_count(), 1);
    s.handle(EngineCommand::NoteOff {
        channel: 0,
        note: 38,
    });
    assert_eq!(
        s.active_voice_count(),
        0,
        "release_secs 0 stops at note-off"
    );

    let mut chromatic = sliced(SliceSelect::Note);
    chromatic.root_note = Some(37);
    let mut s = sampler(&a, &chromatic);
    note_on(&mut s, 37);
    let out = render(&mut s, 4);
    assert_eq!(
        out,
        vec![250.0, 251.0, 252.0, 253.0],
        "root note plays at original pitch"
    );
    let mut s = sampler(&a, &chromatic);
    note_on(&mut s, 37);
    // A transposed note keeps its slice mapping only for the mapped note;
    // pitch semantics come from `increment_for_note` as before.
    let playback = CompiledSamplePlayback::new(&a, &chromatic, 48_000).unwrap();
    assert!((playback.increment_for_note(49) - 2.0).abs() < 1e-9);
}

#[test]
fn random_selection_is_seeded_reproducible_and_panic_resets_it() {
    let a = ramp(1000);
    let mut cfg = sliced(SliceSelect::Random);
    cfg.slices.seed = 42;
    cfg.voices = 1;
    let sequence = |s: &mut RealtimeSampler| {
        (0..12)
            .map(|_| {
                note_on(s, 60);
                s.next_stereo_frame().0
            })
            .collect::<Vec<_>>()
    };
    let run = |cfg: &SampleSettings| sequence(&mut sampler(&a, cfg));
    let first = run(&cfg);
    assert_eq!(first, run(&cfg));
    // Panic rewinds the sequence so a restarted pattern repeats exactly.
    let mut s = sampler(&a, &cfg);
    let _ = sequence(&mut s);
    s.handle(EngineCommand::Panic);
    assert_eq!(sequence(&mut s), first);
    let mut other = cfg.clone();
    other.slices.seed = 43;
    assert_ne!(first, run(&other));
    let starts: std::collections::BTreeSet<_> = first.iter().map(|v| *v as u32).collect();
    assert!(
        starts.len() > 1,
        "random must reach several slices: {starts:?}"
    );
    assert!(starts.iter().all(|v| [0, 250, 500, 750].contains(v)));
}

#[test]
fn step_locks_pick_slices_and_do_not_change_stored_settings() {
    let a = ramp(1000);
    let cfg = sliced(SliceSelect::None);
    let mut s = sampler(&a, &cfg);
    // Without a lock the whole region plays.
    note_on(&mut s, 60);
    assert_eq!(s.next_stereo_frame().0, 0.0);
    s.handle(EngineCommand::Panic);
    s.set_lock_overrides(SampleLockOverrides {
        slice: Some(3),
        ..SampleLockOverrides::default()
    });
    note_on(&mut s, 60);
    assert_eq!(s.next_stereo_frame().0, 500.0);
    s.handle(EngineCommand::Panic);
    // A lock beyond the slice count falls back to the whole region.
    s.set_lock_overrides(SampleLockOverrides {
        slice: Some(99),
        ..SampleLockOverrides::default()
    });
    note_on(&mut s, 60);
    assert_eq!(s.next_stereo_frame().0, 0.0);
    assert_eq!(cfg.slices.select, SliceSelect::None);
    assert_eq!(sample_param_descriptor(SampleParamId::Slice).max, 128.0);
    assert_eq!(
        sample_param_descriptor(SampleParamId::FilterCutoff).name,
        "filter_cutoff"
    );
}

fn tone(frames: usize, rate: u32, hz: f32, amp: f32) -> SampleAsset {
    asset(
        (0..frames)
            .map(|i| {
                let v = amp * (i as f32 * hz * std::f32::consts::TAU / rate as f32).sin();
                StereoFrame::new(v, v)
            })
            .collect(),
        rate,
    )
}

fn rms(values: &[f32]) -> f32 {
    (values.iter().map(|v| v * v).sum::<f32>() / values.len() as f32).sqrt()
}

fn shaped(mutate: impl FnOnce(&mut SampleShaping)) -> SampleSettings {
    let mut s = settings(SampleMode::OneShot);
    mutate(&mut s.sample_shaping);
    s
}

#[test]
fn filter_modes_attenuate_correct_bands_and_off_is_bit_identical() {
    let low = tone(20_000, 48_000, 100.0, 0.8);
    let high = tone(20_000, 48_000, 9_000.0, 0.8);
    let play = |a: &SampleAsset, cfg: &SampleSettings| {
        let mut s = sampler(a, cfg);
        note_on(&mut s, 60);
        render(&mut s, 19_000)[4_000..].to_vec()
    };
    let lp = shaped(|s| {
        s.filter_mode = SampleFilterMode::LowPass;
        s.filter_cutoff = 800.0;
    });
    let hp = shaped(|s| {
        s.filter_mode = SampleFilterMode::HighPass;
        s.filter_cutoff = 800.0;
    });
    let dry_low = rms(&play(&low, &settings(SampleMode::OneShot)));
    let dry_high = rms(&play(&high, &settings(SampleMode::OneShot)));
    assert!(rms(&play(&low, &lp)) > 0.9 * dry_low);
    assert!(rms(&play(&high, &lp)) < 0.1 * dry_high);
    assert!(rms(&play(&high, &hp)) > 0.9 * dry_high);
    assert!(rms(&play(&low, &hp)) < 0.1 * dry_low);

    // Explicit-off shaping with a cutoff set equals the legacy render.
    let mut off = settings(SampleMode::OneShot);
    off.sample_shaping.filter_cutoff = 300.0;
    assert_eq!(
        play(&low, &off)
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>(),
        play(&low, &settings(SampleMode::OneShot))
            .iter()
            .map(|v| v.to_bits())
            .collect::<Vec<_>>()
    );
}

#[test]
fn drive_saturates_and_cutoff_lock_enables_the_filter_for_a_step() {
    let loud = tone(8_000, 48_000, 440.0, 1.0);
    let quiet = tone(8_000, 48_000, 440.0, 0.05);
    let peak = |a: &SampleAsset, drive: f32| {
        let mut cfg = settings(SampleMode::OneShot);
        cfg.sample_shaping.drive_db = drive;
        let mut s = sampler(a, &cfg);
        note_on(&mut s, 60);
        render(&mut s, 7_000)
            .iter()
            .fold(0.0_f32, |m, v| m.max(v.abs()))
    };
    assert!(
        peak(&loud, 18.0) <= 1.0 + 1e-3,
        "normalised tanh never exceeds full scale"
    );
    assert!(
        peak(&quiet, 18.0) > 3.0 * peak(&quiet, 0.0),
        "small signals gain from drive"
    );

    let high = tone(12_000, 48_000, 9_000.0, 0.8);
    let mut s = sampler(&high, &settings(SampleMode::OneShot));
    s.set_lock_overrides(SampleLockOverrides {
        filter_cutoff: Some(300.0),
        ..SampleLockOverrides::default()
    });
    note_on(&mut s, 60);
    let locked = rms(&render(&mut s, 11_000)[3_000..]);
    s.handle(EngineCommand::Panic);
    s.set_lock_overrides(SampleLockOverrides::default());
    note_on(&mut s, 60);
    let free = rms(&render(&mut s, 11_000)[3_000..]);
    assert!(locked < 0.1 * free, "{locked} vs {free}");
}

#[test]
fn transient_shaper_boosts_attacks_and_shortens_tails_with_zero_latency() {
    let rate = 48_000;
    let hit = asset(
        (0..24_000)
            .map(|i| {
                let v = (-(i as f32) / 6_000.0).exp() * (i as f32 * 0.07).sin();
                StereoFrame::new(v, v)
            })
            .collect(),
        rate,
    );
    let measure = |attack: f32, sustain: f32| {
        let mut cfg = settings(SampleMode::OneShot);
        cfg.sample_shaping.transient_attack = attack;
        cfg.sample_shaping.transient_sustain = sustain;
        let mut s = sampler(&hit, &cfg);
        note_on(&mut s, 60);
        let out = render(&mut s, 23_000);
        (rms(&out[0..1_500]), rms(&out[12_000..22_000]))
    };
    let (head, tail) = measure(0.0, 0.0);
    let (head_up, _) = measure(1.0, 0.0);
    let (_, tail_down) = measure(0.0, -1.0);
    let (_, tail_up) = measure(0.0, 1.0);
    assert!(head_up > head * 1.15, "{head_up} vs {head}");
    assert!(tail_down < tail * 0.85, "{tail_down} vs {tail}");
    assert!(tail_up > tail * 1.1, "{tail_up} vs {tail}");

    // Zero lookahead: no output before the input starts.
    let mut delayed = vec![StereoFrame::new(0.0, 0.0); 2_000];
    delayed.extend((0..2_000).map(|i| StereoFrame::new((i as f32 * 0.3).sin(), 0.0)));
    let mut cfg = settings(SampleMode::OneShot);
    cfg.sample_shaping.transient_attack = 1.0;
    let mut s = sampler(&asset(delayed, rate), &cfg);
    note_on(&mut s, 60);
    let out = render(&mut s, 2_000);
    assert!(out.iter().all(|v| *v == 0.0));
}

#[test]
fn envelope_times_match_wall_clock_at_every_sample_rate_and_one_shot_ignores_note_off() {
    for rate in [44_100_u32, 48_000, 96_000] {
        let a = asset(vec![StereoFrame::new(1.0, 1.0); rate as usize * 2], rate);
        let mut cfg = settings(SampleMode::Gate);
        cfg.sample_shaping.envelope = Some(SampleEnvelope {
            attack_secs: 0.1,
            decay_secs: 0.0,
            sustain: 1.0,
            release_secs: 0.1,
        });
        let mut s = RealtimeSampler::new(CompiledSamplePlayback::new(&a, &cfg, rate).unwrap());
        note_on(&mut s, 60);
        let out = render(&mut s, rate as usize / 5);
        let at = |seconds: f32| out[(seconds * rate as f32) as usize];
        assert!((at(0.05) - 0.5).abs() < 0.02, "{rate}: {}", at(0.05));
        assert!((at(0.15) - 1.0).abs() < 1e-3);
        s.handle(EngineCommand::NoteOff {
            channel: 0,
            note: 60,
        });
        let release = render(&mut s, rate as usize / 5);
        assert!(
            (release[(0.05 * rate as f32) as usize] - 0.5).abs() < 0.05,
            "{rate}"
        );
        assert_eq!(s.active_voice_count(), 0);
        assert!(release[(0.15 * rate as f32) as usize].abs() < 1e-6);
    }

    let a = asset(vec![StereoFrame::new(1.0, 1.0); 4_000], 48_000);
    let mut cfg = settings(SampleMode::OneShot);
    cfg.sample_shaping.envelope = Some(SampleEnvelope {
        attack_secs: 0.0,
        decay_secs: 0.0,
        sustain: 1.0,
        release_secs: 0.5,
    });
    let mut s = sampler(&a, &cfg);
    note_on(&mut s, 60);
    s.handle(EngineCommand::NoteOff {
        channel: 0,
        note: 60,
    });
    let out = render(&mut s, 3_999);
    assert!(
        out.iter().all(|v| (*v - 1.0).abs() < 1e-6),
        "one-shot ignores note-off"
    );
}

#[test]
fn legacy_attack_and_release_fades_are_unchanged_without_an_envelope() {
    let a = asset(vec![StereoFrame::new(1.0, 1.0); 2_000], 48_000);
    let mut cfg = settings(SampleMode::Gate);
    cfg.attack_secs = 0.01;
    let mut s = sampler(&a, &cfg);
    note_on(&mut s, 60);
    let out = render(&mut s, 1_000);
    assert_eq!(out[0], 0.0);
    assert!(out[240] > 0.4 && out[240] < 0.6);
    assert_eq!(out[600], 1.0);
}

#[test]
fn live_shaping_updates_glide_without_stopping_voices_and_slices_keep_sounding_regions() {
    let a = tone(48_000, 48_000, 6_000.0, 0.8);
    let cfg = settings(SampleMode::Loop);
    let mut s = sampler(&a, &cfg);
    note_on(&mut s, 60);
    let before = rms(&render(&mut s, 4_800));
    let shaping = SampleShaping {
        filter_mode: SampleFilterMode::LowPass,
        filter_cutoff: 200.0,
        ..SampleShaping::default()
    };
    s.set_shaping(shelloop::CompiledShaping::compile(&shaping, 48_000).unwrap());
    let transition = render(&mut s, 2_000);
    // 5 ms glide: the first sample after the change is not yet fully filtered.
    assert!(transition.iter().all(|v| v.is_finite()));
    assert_eq!(s.active_voice_count(), 1);
    let after = rms(&render(&mut s, 4_800));
    assert!(after < 0.2 * before, "{after} vs {before}");

    // Slice swap does not move a sounding voice.
    let a = ramp(1000);
    let mut s = sampler(&a, &sliced(SliceSelect::Note));
    note_on(&mut s, 37);
    assert_eq!(s.next_stereo_frame().0, 250.0);
    let other = SliceSettings {
        regions: equal_slices(2).unwrap(),
        ..SliceSettings::default()
    };
    s.set_slices(shelloop::SliceTable::compile(&other).unwrap());
    assert_eq!(s.next_stereo_frame().0, 251.0);
    assert_eq!(s.slices().len(), 2);
}

#[test]
fn stealing_and_panic_work_with_slices_and_shaping() {
    let a = ramp(2_000);
    let mut cfg = sliced(SliceSelect::Note);
    cfg.mode = SampleMode::Gate;
    cfg.voices = 2;
    cfg.sample_shaping.filter_mode = SampleFilterMode::LowPass;
    let mut s = sampler(&a, &cfg);
    for note in [36, 37, 38] {
        note_on(&mut s, note);
    }
    assert_eq!(s.active_voice_count(), 2, "third note steals a voice");
    s.handle(EngineCommand::Panic);
    assert_eq!(s.active_voice_count(), 0);
    assert!(render(&mut s, 64).iter().all(|v| *v == 0.0));
}

#[test]
fn fuzzed_settings_stay_finite_and_bounded() {
    let a = tone(6_000, 44_100, 1_234.0, 1.0);
    let mut x = 0x1234_5678_u32;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        (x >> 8) as f32 / 16_777_216.0
    };
    for _ in 0..200 {
        let mut cfg = settings(match (next() * 3.0) as u32 {
            0 => SampleMode::OneShot,
            1 => SampleMode::Gate,
            _ => SampleMode::Loop,
        });
        cfg.sample_shaping.filter_mode = match (next() * 3.0) as u32 {
            0 => SampleFilterMode::Off,
            1 => SampleFilterMode::LowPass,
            _ => SampleFilterMode::HighPass,
        };
        cfg.sample_shaping.filter_cutoff = 20.0 + next() * 19_980.0;
        cfg.sample_shaping.filter_resonance = next() * 0.95;
        cfg.sample_shaping.drive_db = next() * 24.0;
        cfg.sample_shaping.transient_attack = next() * 2.0 - 1.0;
        cfg.sample_shaping.transient_sustain = next() * 2.0 - 1.0;
        if next() > 0.5 {
            cfg.sample_shaping.envelope = Some(SampleEnvelope {
                attack_secs: next() * 0.05,
                decay_secs: next() * 0.05,
                sustain: next(),
                release_secs: next() * 0.05,
            });
        }
        cfg.pitch_semitones = (next() * 96.0 - 48.0).round();
        cfg.reverse = next() > 0.5;
        cfg.slices.regions = equal_slices(1 + (next() * 7.0) as usize).unwrap();
        cfg.slices.select = SliceSelect::Random;
        cfg.slices.seed = (next() * 1e6) as u32;
        let mut s = RealtimeSampler::new(CompiledSamplePlayback::new(&a, &cfg, 44_100).unwrap());
        for n in 0..6 {
            note_on(&mut s, 30 + n * 7);
        }
        for i in 0..3_000 {
            let (l, r) = s.next_stereo_frame();
            assert!(
                l.is_finite() && r.is_finite() && l.abs() < 64.0 && r.abs() < 64.0,
                "{cfg:?} {i}"
            );
            if i == 1_000 {
                s.handle(EngineCommand::NoteOff {
                    channel: 0,
                    note: 30,
                });
            }
        }
    }
}

#[test]
fn sample_and_slice_commands_validate_atomically_and_undo() {
    let mut shaping = SampleShaping::default();
    let mut slices = SliceSettings::default();
    fn exec(
        shaping: &mut SampleShaping,
        slices: &mut SliceSettings,
        line: &str,
    ) -> Result<String, String> {
        apply_sample_command(shaping, slices, parse_sample_command(line)?)
    }
    macro_rules! run {
        ($line:expr) => {
            exec(&mut shaping, &mut slices, $line)
        };
    }
    run!("sample filter lowpass 900 0.4").unwrap();
    run!("sample drive 6").unwrap();
    run!("sample envelope 0.01 0.1 0.6 0.3").unwrap();
    run!("sample transient 0.5 -0.25").unwrap();
    run!("slice auto 8").unwrap();
    run!("slice assign note 48").unwrap();
    run!("slice delete 1").unwrap();
    assert_eq!(shaping.filter_cutoff, 900.0);
    assert_eq!(slices.regions.len(), 7);
    assert_eq!((slices.select, slices.base_note), (SliceSelect::Note, 48));
    let (before_shaping, before_slices) = (shaping, slices.clone());
    for bad in [
        "sample drive 99",
        "sample filter lowpass 5",
        "sample filter sideways",
        "slice add 0.9 0.2",
        "slice add 0.3 0.4",
        "slice assign note 200",
        "slice delete 99",
        "slice auto 500",
        "sample envelope 0.1 0.1",
        "sample drive nan",
    ] {
        assert!(run!(bad).is_err(), "{bad}");
    }
    assert_eq!(shaping, before_shaping);
    assert_eq!(slices, before_slices, "failed commands change nothing");
    run!("sample envelope off").unwrap();
    assert!(shaping.envelope.is_none());
}

fn drum_project(dir: &std::path::Path) -> (MultiTrackProject, SampleAssetBank) {
    let path = dir.join("loop.wav");
    let mut writer = hound::WavWriter::create(
        &path,
        hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    for i in 0..48_000 {
        writer.write_sample((i as f32 * 0.5).sin() * 0.5).unwrap();
    }
    writer.finalize().unwrap();
    let pattern = Pattern::new(
        "p",
        1,
        0.0,
        0,
        vec![
            Some(PatternStep {
                note: 36,
                velocity: 1.0,
                gate: 0.5,
                probability: 1.0,
                ratchets: 1,
                microtiming_frames: 0,
            }),
            None,
            None,
            None,
        ],
    )
    .unwrap();
    let mut track = TrackDefinition::new(TrackId(1), "drums", TrackKind::Sample, pattern);
    let mut sample = SampleSettings::new("loop.wav", SampleMode::OneShot);
    sample.slices.regions = equal_slices(4).unwrap();
    track.sample = Some(sample);
    let project = MultiTrackProject::new(3, 120.0, 4, vec![track]);
    let mut bank = SampleAssetBank::new(usize::MAX);
    bank.load_file(&path).unwrap();
    (project, bank)
}

#[test]
fn controller_edits_reach_the_engine_persist_and_undo() {
    let dir = tempfile::tempdir().unwrap();
    let (project, bank) = drum_project(dir.path());
    let ctx = SampleContext {
        assets: &bank,
        base_dir: dir.path(),
    };
    let mut engine = MultiTrackEngine::from_project(48_000, 4, &project, Some(ctx)).unwrap();
    let (sender, receiver) = bounded(8);
    let mut controller = SessionController::new(project, None, 48_000, sender).unwrap();
    let (garbage, _) = bounded(4);
    let mut run = |line: &str| {
        let result = controller.execute(line, engine.position_frame());
        while let Ok(message) = receiver.try_recv() {
            apply_audio_message(&mut engine, message, &garbage);
        }
        result
    };
    assert!(run("slice list").unwrap().contains("1: 0.000-0.250"));
    run("sample filter lowpass 700 0.3").unwrap();
    run("slice assign note 36").unwrap();
    run("slice delete 4").unwrap();
    assert!(
        run("slice add 0.1 0.2").is_err(),
        "overlap with slice 1 is rejected"
    );
    assert!(run("sample drive 50").is_err());
    run("sample undo").unwrap(); // restores slice 4
    assert!(run("slice list").unwrap().contains("4: 0.750-1.000"));
    let saved = controller.project().tracks[0].sample.clone().unwrap();
    assert_eq!(saved.sample_shaping.filter_mode, SampleFilterMode::LowPass);
    assert_eq!(saved.slices.select, SliceSelect::Note);
    assert_eq!(saved.slices.regions.len(), 4);
    let json = serde_json::to_string(controller.project()).unwrap();
    assert!(json.contains("sample_shaping") && json.contains("\"slices\""));
    for _ in 0..4_000 {
        let (l, r) = engine.next_stereo_frame();
        assert!(l.is_finite() && r.is_finite());
    }
}

fn write_hits_wav(path: &std::path::Path) {
    let rate = 48_000u32;
    let mut writer = hound::WavWriter::create(
        path,
        hound::WavSpec {
            channels: 1,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .unwrap();
    let mut frames = vec![0.0f32; rate as usize];
    for hit in [0.0f64, 0.25, 0.5, 0.75] {
        let start = (hit * f64::from(rate)) as usize + 100;
        for i in 0..2_000 {
            frames[start + i] = (-(i as f32) / 400.0).exp() * (i as f32 * 0.3).sin();
        }
    }
    for value in frames {
        writer.write_sample(value).unwrap();
    }
    writer.finalize().unwrap();
}

#[test]
fn slice_detect_parses_with_defaults_and_bounds() {
    use shelloop::{parse_sample_command, SampleCommand};
    assert_eq!(
        parse_sample_command("slice detect").unwrap(),
        SampleCommand::SliceDetect {
            count: 16,
            sensitivity: 0.5
        }
    );
    assert_eq!(
        parse_sample_command("slice detect 8 0.25").unwrap(),
        SampleCommand::SliceDetect {
            count: 8,
            sensitivity: 0.25
        }
    );
    for bad in [
        "slice detect 0",
        "slice detect 2.5",
        "slice detect 4 1.5",
        "slice detect 4 nan",
        "slice detect -1",
    ] {
        assert!(parse_sample_command(bad).is_err(), "{bad}");
    }
}

#[test]
fn controller_slice_detect_is_undoable_and_reports_missing_files() {
    let dir = tempfile::tempdir().unwrap();
    write_hits_wav(&dir.path().join("hits.wav"));
    let pattern = Pattern::new("p", 1, 0.0, 0, vec![None, None, None, None]).unwrap();
    let mut track = TrackDefinition::new(TrackId(1), "drums", TrackKind::Sample, pattern.clone());
    track.sample = Some(SampleSettings::new("hits.wav", SampleMode::OneShot));
    let mut missing = TrackDefinition::new(TrackId(2), "gone", TrackKind::Sample, pattern);
    missing.sample = Some(SampleSettings::new("missing.wav", SampleMode::OneShot));
    let engine_project = MultiTrackProject::new(3, 120.0, 4, vec![track.clone()]);
    let project = MultiTrackProject::new(3, 120.0, 4, vec![track, missing]);
    let (sender, receiver) = bounded(8);
    let mut controller =
        SessionController::new(project, Some(dir.path().join("p.json")), 48_000, sender).unwrap();
    let (garbage, _) = bounded(4);
    let mut bank = SampleAssetBank::new(usize::MAX);
    bank.load_file(dir.path().join("hits.wav")).unwrap();
    let ctx = SampleContext {
        assets: &bank,
        base_dir: dir.path(),
    };
    let mut engine = MultiTrackEngine::from_project(48_000, 4, &engine_project, Some(ctx)).unwrap();
    macro_rules! run {
        ($line:expr) => {{
            let result = controller.execute($line, engine.position_frame());
            while let Ok(message) = receiver.try_recv() {
                apply_audio_message(&mut engine, message, &garbage);
            }
            result
        }};
    }
    run!("slice clear").unwrap();
    let message = run!("slice detect 8 0.6").unwrap();
    assert!(message.contains("detected"), "{message}");
    let detected = controller.project().tracks[0]
        .sample
        .as_ref()
        .unwrap()
        .slices
        .regions
        .len();
    assert!((2..=8).contains(&detected), "found {detected} slices");
    run!("sample undo").unwrap();
    assert!(
        controller.project().tracks[0]
            .sample
            .as_ref()
            .unwrap()
            .slices
            .regions
            .is_empty(),
        "undo restores the previous (cleared) slice list"
    );
    run!("track 2").unwrap();
    assert!(run!("slice detect 4").is_err(), "file missing on track 2");
}

#[test]
fn slice_maps_commit_on_the_next_bar_while_playing_and_immediately_when_paused() {
    let dir = tempfile::tempdir().unwrap();
    let (project, bank) = drum_project(dir.path());
    let ctx = SampleContext {
        assets: &bank,
        base_dir: dir.path(),
    };
    let mut engine = MultiTrackEngine::from_project(48_000, 4, &project, Some(ctx)).unwrap();
    let (sender, receiver) = bounded(8);
    let mut controller = SessionController::new(project, None, 48_000, sender).unwrap();
    let (garbage, _) = bounded(4);
    macro_rules! run {
        ($line:expr) => {{
            let result = controller.execute($line, engine.position_frame());
            while let Ok(message) = receiver.try_recv() {
                apply_audio_message(&mut engine, message, &garbage);
            }
            result
        }};
    }
    let id = TrackId(1);
    assert_eq!(engine.track_slice_count(id), Some(4));
    // Playing at 120 BPM in 4/4: a bar is 96 000 frames.
    for _ in 0..1_000 {
        engine.next_stereo_frame();
    }
    run!("slice auto 2").unwrap();
    assert_eq!(
        engine.track_slice_count(id),
        Some(4),
        "the old map stays live until the bar"
    );
    for _ in 0..(96_000 - 1_000) {
        engine.next_stereo_frame();
    }
    assert_eq!(engine.track_slice_count(id), Some(4), "one frame early");
    engine.next_stereo_frame();
    assert_eq!(engine.track_slice_count(id), Some(2), "commits on the bar");

    // Paused: the clock cannot reach a boundary, so the edit lands at once.
    run!("pause").unwrap();
    run!("slice auto 3").unwrap();
    assert_eq!(engine.track_slice_count(id), Some(3));
}

#[test]
fn queued_slice_map_commits_at_once_when_the_clock_restarts() {
    // A restart discards the timeline the edit was quantised against, so a
    // pending slice map must not wait for a frame the restarted clock would
    // reach only after a full bar.
    let dir = tempfile::tempdir().unwrap();
    let (project, bank) = drum_project(dir.path());
    let ctx = SampleContext {
        assets: &bank,
        base_dir: dir.path(),
    };
    let mut engine = MultiTrackEngine::from_project(48_000, 4, &project, Some(ctx)).unwrap();
    let (sender, receiver) = bounded(8);
    let mut controller = SessionController::new(project, None, 48_000, sender).unwrap();
    let (garbage, _) = bounded(4);
    macro_rules! run {
        ($line:expr) => {{
            let result = controller.execute($line, engine.position_frame());
            while let Ok(message) = receiver.try_recv() {
                apply_audio_message(&mut engine, message, &garbage);
            }
            result
        }};
    }
    for _ in 0..1_000 {
        engine.next_stereo_frame();
    }
    run!("slice auto 2").unwrap();
    assert_eq!(engine.track_slice_count(TrackId(1)), Some(4));
    run!("restart").unwrap();
    assert_eq!(
        engine.track_slice_count(TrackId(1)),
        Some(2),
        "restart commits the pending map"
    );
}

#[test]
fn immediate_sample_configuration_api_remains_compatible() {
    let dir = tempfile::tempdir().unwrap();
    let (project, bank) = drum_project(dir.path());
    let context = SampleContext {
        assets: &bank,
        base_dir: dir.path(),
    };
    let mut engine = MultiTrackEngine::from_project(48_000, 4, &project, Some(context)).unwrap();
    let shaping = CompiledShaping::compile(&SampleShaping::default(), 48_000).unwrap();
    let mut settings = SliceSettings {
        regions: equal_slices(2).unwrap(),
        ..SliceSettings::default()
    };
    let slices = SliceTable::compile(&settings).unwrap();
    engine
        .apply_sample_config(TrackId(1), shaping, slices)
        .unwrap();
    assert_eq!(engine.track_slice_count(TrackId(1)), Some(2));
    settings.regions = equal_slices(3).unwrap();
    let (garbage, _) = bounded(4);
    apply_audio_message(
        &mut engine,
        shelloop::AudioMessage::SampleConfig {
            track: TrackId(1),
            shaping,
            slices: SliceTable::compile(&settings).unwrap(),
        },
        &garbage,
    );
    assert_eq!(engine.track_slice_count(TrackId(1)), Some(3));
}
