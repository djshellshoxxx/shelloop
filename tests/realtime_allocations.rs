//! Allocation-free guarantees for the real-time render paths added by specs
//! 13-15. Runs as the only test in its binary so the counting allocator is
//! not polluted by other threads.

use shelloop::{
    CompiledSynthPatch, Effect, EffectConfig, EffectKind, EngineCommand, ModDest, ModRoute,
    ModSource, Oscillator, RealtimeSynth, SynthPatch,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Counting;
static ALLOCATIONS: AtomicUsize = AtomicUsize::new(0);
thread_local! {
    static MEASURING: Cell<bool> = const { Cell::new(false) };
}

fn count_allocation() {
    if MEASURING.try_with(Cell::get).unwrap_or(false) {
        ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
    }
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation();
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count_allocation();
        System.realloc(ptr, layout, new_size)
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn allocations_during(f: impl FnOnce()) -> usize {
    struct Measurement(bool);
    impl Drop for Measurement {
        fn drop(&mut self) {
            MEASURING.with(|flag| flag.set(self.0));
        }
    }
    let guard = Measurement(MEASURING.with(|flag| flag.replace(true)));
    let before = ALLOCATIONS.load(Ordering::SeqCst);
    f();
    let count = ALLOCATIONS.load(Ordering::SeqCst) - before;
    drop(guard);
    count
}

#[test]
fn warmed_renders_do_not_allocate() {
    allocation_probe_is_scoped_to_the_render_thread();
    sample_shaping_does_not_allocate();
    modulation_and_effects_do_not_allocate();
}

fn allocation_probe_is_scoped_to_the_render_thread() {
    use std::sync::{atomic::AtomicBool, Arc};
    assert!(
        allocations_during(|| {
            std::hint::black_box(Box::new([0_u8; 1_024]));
        }) >= 1,
        "the probe must detect allocations on the measured thread"
    );
    let start = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let (worker_start, worker_done) = (start.clone(), done.clone());
    let worker = std::thread::spawn(move || {
        while !worker_start.load(Ordering::SeqCst) {
            std::hint::spin_loop();
        }
        std::hint::black_box(Box::new([0_u8; 1_024]));
        worker_done.store(true, Ordering::SeqCst);
    });
    let count = allocations_during(|| {
        start.store(true, Ordering::SeqCst);
        while !done.load(Ordering::SeqCst) {
            std::hint::spin_loop();
        }
    });
    worker.join().unwrap();
    assert_eq!(
        count, 0,
        "unrelated control/harness thread allocations must not pollute the audio probe"
    );
}

fn sample_shaping_does_not_allocate() {
    use shelloop::{
        equal_slices, CompiledSamplePlayback, CompiledShaping, RealtimeSampler, SampleAsset,
        SampleAssetId, SampleEnvelope, SampleFilterMode, SampleMode, SampleSettings, SampleShaping,
        SliceSelect, SliceTable, StereoFrame,
    };
    let asset = SampleAsset {
        id: SampleAssetId(0),
        source_path: std::path::PathBuf::from("x.wav"),
        source_sample_rate: 48_000,
        channels: 1,
        frames: (0..48_000)
            .map(|i| StereoFrame::new((i as f32 * 0.05).sin(), (i as f32 * 0.07).cos()))
            .collect::<Vec<_>>()
            .into(),
    };
    let mut settings = SampleSettings::new("x.wav", SampleMode::Loop);
    settings.slices.regions = equal_slices(8).unwrap();
    settings.slices.select = SliceSelect::Random;
    settings.sample_shaping.filter_mode = SampleFilterMode::LowPass;
    settings.sample_shaping.filter_cutoff = 2_000.0;
    settings.sample_shaping.drive_db = 6.0;
    settings.sample_shaping.transient_attack = 0.5;
    settings.sample_shaping.envelope = Some(SampleEnvelope {
        attack_secs: 0.01,
        decay_secs: 0.05,
        sustain: 0.7,
        release_secs: 0.1,
    });
    let mut sampler =
        RealtimeSampler::new(CompiledSamplePlayback::new(&asset, &settings, 48_000).unwrap());
    let shaping = SampleShaping {
        filter_mode: SampleFilterMode::HighPass,
        ..SampleShaping::default()
    };
    let compiled = CompiledShaping::compile(&shaping, 48_000).unwrap();
    let table = SliceTable::compile(&settings.slices).unwrap();
    for note in 36..40 {
        sampler.handle(EngineCommand::NoteOn {
            channel: 0,
            note,
            velocity: 1.0,
        });
    }
    for _ in 0..500 {
        sampler.next_stereo_frame();
    }
    assert_eq!(
        allocations_during(|| {
            for note in 40..46 {
                sampler.handle(EngineCommand::NoteOn {
                    channel: 0,
                    note,
                    velocity: 1.0,
                });
            }
            sampler.set_shaping(compiled);
            sampler.set_slices(table);
            for note in 36..40 {
                sampler.handle(EngineCommand::NoteOff { channel: 0, note });
            }
            for _ in 0..30_000 {
                std::hint::black_box(sampler.next_stereo_frame());
            }
            sampler.handle(EngineCommand::Panic);
        }),
        0,
        "shaped sample playback must not allocate"
    );
}

fn modulation_and_effects_do_not_allocate() {
    // Modulated synth: note-on, patch swap and 20k samples.
    let mut patch = SynthPatch::legacy(Oscillator::Saw);
    patch.filter.mode = shelloop::FilterMode::LowPass;
    patch.filter.cutoff_hz = 2_000.0;
    patch.modulation.lfos[0].rate_hz = 7.0;
    for (source, dest) in [
        (ModSource::Lfo1, ModDest::FilterCutoff),
        (ModSource::Lfo2, ModDest::Tuning),
        (ModSource::Velocity, ModDest::Amplitude),
        (ModSource::Macro1, ModDest::Pan),
    ] {
        patch
            .modulation
            .add_route(ModRoute::new(source, dest, 0.4))
            .unwrap();
    }
    let mut synth = RealtimeSynth::new_with_patch(48_000.0, patch, 8).unwrap();
    let swapped = {
        let mut p = patch;
        p.modulation.macros = [0.5, 0.0];
        p
    };
    let compiled = CompiledSynthPatch::new(48_000.0, swapped).unwrap();
    for note in 48..52 {
        synth.handle(EngineCommand::NoteOn {
            channel: 0,
            note,
            velocity: 0.8,
        });
    }
    for _ in 0..1_000 {
        synth.next_sample();
    }
    assert_eq!(
        allocations_during(|| {
            for note in 52..56 {
                synth.handle(EngineCommand::NoteOn {
                    channel: 0,
                    note,
                    velocity: 0.8,
                });
            }
            synth.handle(EngineCommand::ModWheel { value: 0.5 });
            let compiled = CompiledSynthPatch::new(48_000.0, swapped).unwrap();
            assert!(synth.apply_patch(compiled));
            for _ in 0..20_000 {
                std::hint::black_box(synth.next_sample());
            }
            synth.handle(EngineCommand::Panic);
        }),
        0,
        "modulated synth rendering must not allocate"
    );
    let _ = compiled;

    // Creative effects: process and parameter updates after construction.
    for kind in [
        EffectKind::Filter,
        EffectKind::Chorus,
        EffectKind::BeatRepeat,
        EffectKind::BitCrusher,
    ] {
        let mut fx = Effect::new(&EffectConfig::new(kind), 48_000, 120.0).unwrap();
        let active = shelloop::find_effect_param(
            kind,
            if kind == EffectKind::BeatRepeat {
                "active"
            } else {
                "mix"
            },
        )
        .unwrap();
        for n in 0..5_000 {
            fx.process((n as f32 * 0.01).sin(), 0.0);
        }
        assert_eq!(
            allocations_during(|| {
                fx.set_param(active, 1.0);
                fx.set_tempo(140.0);
                for n in 0..60_000 {
                    std::hint::black_box(fx.process_with_transport(
                        (n as f32 * 0.01).sin(),
                        (n as f32 * 0.02).cos(),
                        n,
                        true,
                    ));
                }
                fx.set_bypassed(true);
                for n in 0..2_000 {
                    std::hint::black_box(fx.process(n as f32, 0.0));
                }
                fx.reset();
            }),
            0,
            "{kind:?} must not allocate while processing"
        );
    }
}
