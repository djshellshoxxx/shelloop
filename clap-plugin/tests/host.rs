//! A minimal in-process CLAP host that drives the plugin through the
//! exported `clap_entry`, exactly as a DAW would after `dlopen`.

use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::events::{
    clap_event_header, clap_event_midi, clap_event_note, clap_event_param_value,
    clap_event_transport, clap_input_events, clap_output_events, CLAP_CORE_EVENT_SPACE_ID,
    CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_VALUE,
    CLAP_TRANSPORT_HAS_BEATS_TIMELINE, CLAP_TRANSPORT_HAS_TEMPO, CLAP_TRANSPORT_IS_PLAYING,
};
use clap_sys::ext::audio_ports::{
    clap_audio_port_info, clap_plugin_audio_ports, CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS,
};
use clap_sys::ext::latency::{clap_plugin_latency, CLAP_EXT_LATENCY};
use clap_sys::ext::note_ports::{
    clap_note_port_info, clap_plugin_note_ports, CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP,
    CLAP_NOTE_DIALECT_MIDI,
};
use clap_sys::ext::params::{
    clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS, CLAP_PARAM_IS_AUTOMATABLE,
    CLAP_PARAM_IS_ENUM, CLAP_PARAM_IS_STEPPED,
};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::fixedpoint::CLAP_BEATTIME_FACTOR;
use clap_sys::host::clap_host;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{clap_process, CLAP_PROCESS_CONTINUE};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::CLAP_VERSION;
use shelloop_clap::clap_entry;
use shelloop_clap::params::ParamId;
use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr::{null, null_mut};

const SAMPLE_RATE: f64 = 48_000.0;
const BLOCK: usize = 512;

// ---------------------------------------------------------------------------
// Fake host
// ---------------------------------------------------------------------------

unsafe extern "C" fn host_get_extension(
    _host: *const clap_host,
    _id: *const c_char,
) -> *const c_void {
    null()
}
unsafe extern "C" fn host_noop(_host: *const clap_host) {}

fn fake_host() -> Box<clap_host> {
    Box::new(clap_host {
        clap_version: CLAP_VERSION,
        host_data: null_mut(),
        name: c"shelloop test host".as_ptr(),
        vendor: c"tests".as_ptr(),
        url: c"".as_ptr(),
        version: c"0".as_ptr(),
        get_extension: Some(host_get_extension),
        request_restart: Some(host_noop),
        request_process: Some(host_noop),
        request_callback: Some(host_noop),
    })
}

/// Any CLAP event the test sends, stored with a stable address.
enum Event {
    Note(clap_event_note),
    Midi(clap_event_midi),
    Param(clap_event_param_value),
}

impl Event {
    fn header(&self) -> *const clap_event_header {
        match self {
            Event::Note(e) => &e.header,
            Event::Midi(e) => &e.header,
            Event::Param(e) => &e.header,
        }
    }
}

fn header(type_: u16, size: usize, time: u32) -> clap_event_header {
    clap_event_header {
        size: size as u32,
        time,
        space_id: CLAP_CORE_EVENT_SPACE_ID,
        type_,
        flags: 0,
    }
}

fn note(type_: u16, time: u32, key: i16, velocity: f64) -> Event {
    Event::Note(clap_event_note {
        header: header(type_, size_of::<clap_event_note>(), time),
        note_id: -1,
        port_index: 0,
        channel: 0,
        key,
        velocity,
    })
}

fn midi(time: u32, data: [u8; 3]) -> Event {
    Event::Midi(clap_event_midi {
        header: header(CLAP_EVENT_MIDI, size_of::<clap_event_midi>(), time),
        port_index: 0,
        data,
    })
}

fn param(time: u32, id: ParamId, value: f64) -> Event {
    Event::Param(clap_event_param_value {
        header: header(
            CLAP_EVENT_PARAM_VALUE,
            size_of::<clap_event_param_value>(),
            time,
        ),
        param_id: id.raw(),
        cookie: null_mut(),
        note_id: -1,
        port_index: -1,
        channel: -1,
        key: -1,
        value,
    })
}

unsafe extern "C" fn events_size(list: *const clap_input_events) -> u32 {
    let events = &*((*list).ctx as *const Vec<Event>);
    events.len() as u32
}

unsafe extern "C" fn events_get(
    list: *const clap_input_events,
    index: u32,
) -> *const clap_event_header {
    let events = &*((*list).ctx as *const Vec<Event>);
    events
        .get(index as usize)
        .map_or(null(), |event| event.header())
}

unsafe extern "C" fn out_events_push(
    _list: *const clap_output_events,
    _event: *const clap_event_header,
) -> bool {
    true
}

fn transport(playing: bool, beats: f64, tempo: f64) -> clap_event_transport {
    let mut flags = CLAP_TRANSPORT_HAS_TEMPO | CLAP_TRANSPORT_HAS_BEATS_TIMELINE;
    if playing {
        flags |= CLAP_TRANSPORT_IS_PLAYING;
    }
    clap_event_transport {
        header: header(9, size_of::<clap_event_transport>(), 0),
        flags,
        song_pos_beats: (beats * CLAP_BEATTIME_FACTOR as f64) as i64,
        song_pos_seconds: 0,
        tempo,
        tempo_inc: 0.0,
        loop_start_beats: 0,
        loop_end_beats: 0,
        loop_start_seconds: 0,
        loop_end_seconds: 0,
        bar_start: 0,
        bar_number: 0,
        tsig_num: 4,
        tsig_denom: 4,
    }
}

/// A created and initialised plugin instance plus the host that owns it.
struct Instance {
    plugin: *const clap_plugin,
    _host: Box<clap_host>,
    steady_time: i64,
}

fn factory() -> &'static clap_plugin_factory {
    unsafe {
        let factory = clap_entry.get_factory.unwrap()(CLAP_PLUGIN_FACTORY_ID.as_ptr());
        assert!(!factory.is_null());
        &*(factory as *const clap_plugin_factory)
    }
}

impl Instance {
    fn new() -> Self {
        let host = fake_host();
        let id = CString::new(shelloop_clap::PLUGIN_ID).unwrap();
        let plugin = unsafe { factory().create_plugin.unwrap()(factory(), &*host, id.as_ptr()) };
        assert!(!plugin.is_null());
        assert!(unsafe { (*plugin).init.unwrap()(plugin) });
        Self {
            plugin,
            _host: host,
            steady_time: 0,
        }
    }

    fn ext<T>(&self, id: &CStr) -> &'static T {
        unsafe {
            let ext = (*self.plugin).get_extension.unwrap()(self.plugin, id.as_ptr());
            assert!(!ext.is_null(), "missing extension {id:?}");
            &*(ext as *const T)
        }
    }

    fn params(&self) -> &'static clap_plugin_params {
        self.ext(CLAP_EXT_PARAMS)
    }

    fn activate(&self) {
        unsafe {
            assert!((*self.plugin).activate.unwrap()(
                self.plugin,
                SAMPLE_RATE,
                1,
                BLOCK as u32
            ));
            assert!((*self.plugin).start_processing.unwrap()(self.plugin));
        }
    }

    fn process(
        &mut self,
        events: Vec<Event>,
        transport: Option<&clap_event_transport>,
    ) -> [Vec<f32>; 2] {
        let mut left = vec![f32::NAN; BLOCK];
        let mut right = vec![f32::NAN; BLOCK];
        let mut channels = [left.as_mut_ptr(), right.as_mut_ptr()];
        let mut output = clap_audio_buffer {
            data32: channels.as_mut_ptr(),
            data64: null_mut(),
            channel_count: 2,
            latency: 0,
            constant_mask: 0,
        };
        let in_events = clap_input_events {
            ctx: &events as *const Vec<Event> as *mut c_void,
            size: Some(events_size),
            get: Some(events_get),
        };
        let out_events = clap_output_events {
            ctx: null_mut(),
            try_push: Some(out_events_push),
        };
        let process = clap_process {
            steady_time: self.steady_time,
            frames_count: BLOCK as u32,
            transport: transport.map_or(null(), |t| t as *const _),
            audio_inputs: null(),
            audio_outputs: &mut output,
            audio_inputs_count: 0,
            audio_outputs_count: 1,
            in_events: &in_events,
            out_events: &out_events,
        };
        let status = unsafe { (*self.plugin).process.unwrap()(self.plugin, &process) };
        assert_eq!(status, CLAP_PROCESS_CONTINUE);
        self.steady_time += BLOCK as i64;
        [left, right]
    }

    fn param_value(&self, id: ParamId) -> f64 {
        let mut value = f64::NAN;
        assert!(unsafe { self.params().get_value.unwrap()(self.plugin, id.raw(), &mut value) });
        value
    }

    /// Set parameters through `params.flush`, as a host does while inactive.
    fn flush(&self, events: Vec<Event>) {
        let in_events = clap_input_events {
            ctx: &events as *const Vec<Event> as *mut c_void,
            size: Some(events_size),
            get: Some(events_get),
        };
        let out_events = clap_output_events {
            ctx: null_mut(),
            try_push: Some(out_events_push),
        };
        unsafe { self.params().flush.unwrap()(self.plugin, &in_events, &out_events) };
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            (*self.plugin).stop_processing.unwrap()(self.plugin);
            (*self.plugin).deactivate.unwrap()(self.plugin);
            (*self.plugin).destroy.unwrap()(self.plugin);
        }
    }
}

fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0_f32, |max, s| max.max(s.abs()))
}

fn with_entry(test: impl FnOnce()) {
    unsafe {
        assert!(clap_entry.init.unwrap()(c"/tmp/shelloop.clap".as_ptr()));
    }
    test();
    unsafe { clap_entry.deinit.unwrap()() };
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn entry_exposes_one_descriptor() {
    with_entry(|| unsafe {
        assert!(clap_entry.clap_version.major >= 1);
        assert!(clap_entry.get_factory.unwrap()(c"unknown.factory".as_ptr()).is_null());
        let factory = factory();
        assert_eq!(factory.get_plugin_count.unwrap()(factory), 1);
        let desc = &*factory.get_plugin_descriptor.unwrap()(factory, 0);
        assert_eq!(
            CStr::from_ptr(desc.id).to_str().unwrap(),
            "com.circuitdriftlabs.shelloop"
        );
        assert_eq!(CStr::from_ptr(desc.name).to_str().unwrap(), "SHELLOOP");
        assert_eq!(
            CStr::from_ptr(desc.vendor).to_str().unwrap(),
            "Circuit Drift Labs"
        );
        assert_eq!(
            CStr::from_ptr(desc.url).to_str().unwrap(),
            "https://github.com/djshellshoxxx/shelloop"
        );
        assert_eq!(
            CStr::from_ptr(desc.version).to_str().unwrap(),
            env!("CARGO_PKG_VERSION")
        );
        let mut features = Vec::new();
        let mut cursor = desc.features;
        while !(*cursor).is_null() {
            features.push(CStr::from_ptr(*cursor).to_str().unwrap());
            cursor = cursor.add(1);
        }
        assert_eq!(features, ["instrument", "synthesizer", "stereo"]);
        assert!(factory.get_plugin_descriptor.unwrap()(factory, 1).is_null());

        let host = fake_host();
        assert!(
            factory.create_plugin.unwrap()(factory, &*host, c"other.plugin".as_ptr()).is_null()
        );
    });
}

#[test]
fn extensions_describe_ports_params_and_latency() {
    with_entry(|| unsafe {
        let instance = Instance::new();
        let plugin = instance.plugin;

        let notes: &clap_plugin_note_ports = instance.ext(CLAP_EXT_NOTE_PORTS);
        assert_eq!(notes.count.unwrap()(plugin, true), 1);
        assert_eq!(notes.count.unwrap()(plugin, false), 0);
        let mut note_info: clap_note_port_info = std::mem::zeroed();
        assert!(notes.get.unwrap()(plugin, 0, true, &mut note_info));
        assert_eq!(
            note_info.supported_dialects,
            CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI
        );
        assert_eq!(note_info.preferred_dialect, CLAP_NOTE_DIALECT_CLAP);

        let audio: &clap_plugin_audio_ports = instance.ext(CLAP_EXT_AUDIO_PORTS);
        assert_eq!(audio.count.unwrap()(plugin, true), 0);
        assert_eq!(audio.count.unwrap()(plugin, false), 1);
        let mut audio_info: clap_audio_port_info = std::mem::zeroed();
        assert!(audio.get.unwrap()(plugin, 0, false, &mut audio_info));
        assert_eq!(audio_info.channel_count, 2);
        assert_eq!(audio_info.flags, CLAP_AUDIO_PORT_IS_MAIN);
        assert_eq!(
            CStr::from_ptr(audio_info.port_type).to_str().unwrap(),
            "stereo"
        );

        let latency: &clap_plugin_latency = instance.ext(CLAP_EXT_LATENCY);
        assert_eq!(latency.get.unwrap()(plugin), 0);
        let _: &clap_plugin_state = instance.ext(CLAP_EXT_STATE);
        assert!((*plugin).get_extension.unwrap()(plugin, c"clap.gui".as_ptr()).is_null());

        let params = instance.params();
        let count = params.count.unwrap()(plugin);
        assert_eq!(count, 15);
        let mut names = Vec::new();
        for index in 0..count {
            let mut info: clap_param_info = std::mem::zeroed();
            assert!(params.get_info.unwrap()(plugin, index, &mut info));
            assert_eq!(info.id, index, "ids are stable and dense");
            assert!(info.flags & CLAP_PARAM_IS_AUTOMATABLE != 0);
            assert!(info.min_value <= info.default_value && info.default_value <= info.max_value);
            assert_eq!(
                instance.param_value(ParamId::from_raw(info.id).unwrap()),
                info.default_value
            );
            let name = CStr::from_ptr(info.name.as_ptr())
                .to_str()
                .unwrap()
                .to_owned();
            let stepped = info.flags & CLAP_PARAM_IS_STEPPED != 0;
            let is_enum = info.flags & CLAP_PARAM_IS_ENUM != 0;
            match name.as_str() {
                "Oscillator" | "Filter mode" | "Sequencer" => assert!(stepped && is_enum),
                "Octave" => assert!(stepped && !is_enum),
                _ => assert!(!stepped && !is_enum, "{name}"),
            }
            names.push(name);
        }
        assert_eq!(
            names,
            [
                "Oscillator",
                "Octave",
                "Fine",
                "Pulse width",
                "Amp attack",
                "Amp decay",
                "Amp sustain",
                "Amp release",
                "Filter mode",
                "Cutoff",
                "Resonance",
                "Filter env amount",
                "Output gain",
                "Sequencer",
                "Master volume",
            ]
        );
        let mut info: clap_param_info = std::mem::zeroed();
        assert!(!params.get_info.unwrap()(plugin, count, &mut info));
    });
}

#[test]
fn note_on_is_sample_accurate_and_note_off_decays() {
    with_entry(|| {
        let mut instance = Instance::new();
        instance.activate();

        let [left, right] = instance.process(vec![note(CLAP_EVENT_NOTE_ON, 100, 57, 0.9)], None);
        assert!(
            left[..100].iter().all(|s| *s == 0.0),
            "silence before the note"
        );
        assert!(left.iter().chain(&right).all(|s| s.is_finite()));
        assert!(peak(&left[100..]) > 0.01, "audible after the note");
        assert_eq!(left, right, "mono synth on both channels");

        let [held, _] = instance.process(Vec::new(), None);
        assert!(peak(&held) > 0.01);

        instance.process(vec![note(CLAP_EVENT_NOTE_OFF, 0, 57, 0.0)], None);
        // Default release is 250 ms; give it half a second.
        let mut last = Vec::new();
        for _ in 0..48 {
            last = instance.process(Vec::new(), None)[0].clone();
        }
        assert!(peak(&last) < 1e-6, "voice decayed, peak {}", peak(&last));
    });
}

#[test]
fn midi_dialect_and_param_automation_work_in_process() {
    with_entry(|| {
        let mut instance = Instance::new();
        instance.activate();
        let [out, _] = instance.process(
            vec![
                param(0, ParamId::Oscillator, 0.0),
                param(0, ParamId::FilterMode, 0.0),
                midi(10, [0x90, 69, 100]),
                param(300, ParamId::MasterVolume, 0.0),
            ],
            None,
        );
        assert!(out[..10].iter().all(|s| *s == 0.0));
        assert!(peak(&out[10..300]) > 0.01);
        assert_eq!(instance.param_value(ParamId::Oscillator), 0.0);
        assert_eq!(instance.param_value(ParamId::MasterVolume), 0.0);
        // Master volume is smoothed (5 ms time constant): silent within ~50 ms.
        let mut later = Vec::new();
        for _ in 0..5 {
            later = instance.process(Vec::new(), None)[0].clone();
        }
        assert!(peak(&later) < 1e-3, "peak {}", peak(&later));

        // CC123 silences everything.
        instance.process(
            vec![
                param(0, ParamId::MasterVolume, 1.0),
                midi(0, [0xB0, 123, 0]),
            ],
            None,
        );
        let [after_panic, _] = instance.process(Vec::new(), None);
        assert!(after_panic.iter().all(|s| *s == 0.0));
    });
}

#[test]
fn params_text_round_trip() {
    with_entry(|| unsafe {
        let instance = Instance::new();
        let params = instance.params();
        let plugin = instance.plugin;
        let cases = [
            (ParamId::Oscillator, 3.0, "Pulse"),
            (ParamId::FilterMode, 2.0, "High-pass"),
            (ParamId::Sequencer, 1.0, "On"),
            (ParamId::Octave, -2.0, "-2"),
            (ParamId::Cutoff, 2500.0, "2.50 kHz"),
            (ParamId::AmpAttack, 0.25, "250.0 ms"),
            (ParamId::AmpSustain, 0.5, "50.0 %"),
            (ParamId::Fine, 12.0, "+12.0 ct"),
            (ParamId::FilterEnvAmount, -3.5, "-3.50 oct"),
            (ParamId::MasterVolume, 1.25, "1.250"),
        ];
        for (id, value, expected) in cases {
            let mut buffer = [0 as c_char; 64];
            assert!(params.value_to_text.unwrap()(
                plugin,
                id.raw(),
                value,
                buffer.as_mut_ptr(),
                buffer.len() as u32
            ));
            let text = CStr::from_ptr(buffer.as_ptr());
            assert_eq!(text.to_str().unwrap(), expected);
            let mut parsed = f64::NAN;
            assert!(params.text_to_value.unwrap()(
                plugin,
                id.raw(),
                text.as_ptr(),
                &mut parsed
            ));
            assert!((parsed - value).abs() < 1e-9, "{expected} -> {parsed}");
        }

        // Truncation into a tiny buffer still NUL-terminates.
        let mut tiny = [1 as c_char; 4];
        assert!(params.value_to_text.unwrap()(
            plugin,
            ParamId::Oscillator.raw(),
            1.0,
            tiny.as_mut_ptr(),
            4
        ));
        assert_eq!(CStr::from_ptr(tiny.as_ptr()).to_str().unwrap(), "Tri");

        let mut out = 0.0;
        assert!(!params.text_to_value.unwrap()(
            plugin,
            ParamId::Cutoff.raw(),
            c"bright".as_ptr(),
            &mut out
        ));
        assert!(!params.text_to_value.unwrap()(
            plugin,
            999,
            c"1".as_ptr(),
            &mut out
        ));
    });
}

/// Output stream that accepts at most 7 bytes per call (partial writes).
unsafe extern "C" fn ostream_write(
    stream: *const clap_ostream,
    buffer: *const c_void,
    size: u64,
) -> i64 {
    let sink = &mut *((*stream).ctx as *mut Vec<u8>);
    let count = size.min(7) as usize;
    sink.extend_from_slice(std::slice::from_raw_parts(buffer as *const u8, count));
    count as i64
}

struct Source {
    data: Vec<u8>,
    position: usize,
}

/// Input stream that returns at most 5 bytes per call (partial reads).
unsafe extern "C" fn istream_read(
    stream: *const clap_istream,
    buffer: *mut c_void,
    size: u64,
) -> i64 {
    let source = &mut *((*stream).ctx as *mut Source);
    let remaining = &source.data[source.position..];
    let count = remaining.len().min(size as usize).min(5);
    std::ptr::copy_nonoverlapping(remaining.as_ptr(), buffer as *mut u8, count);
    source.position += count;
    count as i64
}

#[test]
fn state_save_and_load_into_second_instance() {
    with_entry(|| unsafe {
        let first = Instance::new();
        first.flush(vec![
            param(0, ParamId::Oscillator, 3.0),
            param(0, ParamId::Octave, -1.0),
            param(0, ParamId::Cutoff, 640.0),
            param(0, ParamId::AmpRelease, 1.5),
            param(0, ParamId::Sequencer, 1.0),
            param(0, ParamId::MasterVolume, 0.6),
        ]);
        assert_eq!(first.param_value(ParamId::Cutoff), 640.0);

        let mut blob = Vec::<u8>::new();
        let ostream = clap_ostream {
            ctx: &mut blob as *mut Vec<u8> as *mut c_void,
            write: Some(ostream_write),
        };
        let state: &clap_plugin_state = first.ext(CLAP_EXT_STATE);
        assert!(state.save.unwrap()(first.plugin, &ostream));
        assert!(blob.len() > 7, "partial writes were continued");
        let json: serde_json::Value = serde_json::from_slice(&blob).unwrap();
        assert_eq!(json["version"], 1);

        let mut second = Instance::new();
        second.activate();
        let mut source = Source {
            data: blob,
            position: 0,
        };
        let istream = clap_istream {
            ctx: &mut source as *mut Source as *mut c_void,
            read: Some(istream_read),
        };
        let state2: &clap_plugin_state = second.ext(CLAP_EXT_STATE);
        assert!(state2.load.unwrap()(second.plugin, &istream));
        for id in ParamId::ALL {
            assert_eq!(second.param_value(id), first.param_value(id), "{id:?}");
        }
        // The active instance picks the loaded values up on its next block.
        second.process(Vec::new(), None);

        // Garbage is rejected and leaves values untouched.
        let mut bad = Source {
            data: b"{\"format\":\"nope\"}".to_vec(),
            position: 0,
        };
        let bad_stream = clap_istream {
            ctx: &mut bad as *mut Source as *mut c_void,
            read: Some(istream_read),
        };
        assert!(!state2.load.unwrap()(second.plugin, &bad_stream));
        assert_eq!(second.param_value(ParamId::Cutoff), 640.0);
    });
}

#[test]
fn sequencer_plays_with_host_transport_and_stops_cleanly() {
    with_entry(|| {
        let mut instance = Instance::new();
        instance.flush(vec![param(0, ParamId::Sequencer, 1.0)]);
        instance.activate();

        // Stopped transport: nothing plays.
        let stopped = transport(false, 0.0, 120.0);
        let [silent, _] = instance.process(Vec::new(), Some(&stopped));
        assert!(silent.iter().all(|s| *s == 0.0));

        // Playing at 120 BPM from beat 0 for one bar (2 s = ~188 blocks).
        let mut beats = 0.0;
        let mut total_peak = 0.0_f32;
        for _ in 0..188 {
            let playing = transport(true, beats, 120.0);
            let [out, _] = instance.process(Vec::new(), Some(&playing));
            assert!(out.iter().all(|s| s.is_finite()));
            total_peak = total_peak.max(peak(&out));
            beats += BLOCK as f64 / SAMPLE_RATE * 2.0;
        }
        assert!(total_peak > 0.01, "sequencer produced audio without notes");

        // Stop: sequencer notes are released and the output decays to silence.
        let stopped = transport(false, beats, 120.0);
        let mut last = Vec::new();
        for _ in 0..60 {
            last = instance.process(Vec::new(), Some(&stopped))[0].clone();
        }
        assert!(peak(&last) < 1e-6, "no hanging notes, peak {}", peak(&last));

        // Sequencer param off while playing: silent.
        instance.process(vec![param(0, ParamId::Sequencer, 0.0)], None);
        let mut quiet_peak = 0.0_f32;
        for block in 0..60 {
            let playing = transport(true, block as f64 * 0.02, 120.0);
            let [out, _] = instance.process(Vec::new(), Some(&playing));
            quiet_peak = quiet_peak.max(peak(&out));
        }
        assert_eq!(quiet_peak, 0.0);
    });
}

#[test]
fn reset_and_reactivation_are_clean() {
    with_entry(|| unsafe {
        let mut instance = Instance::new();
        instance.activate();
        instance.process(vec![note(CLAP_EVENT_NOTE_ON, 0, 60, 1.0)], None);
        (*instance.plugin).reset.unwrap()(instance.plugin);
        let [out, _] = instance.process(Vec::new(), None);
        assert!(out.iter().all(|s| *s == 0.0));

        // Deactivate, reactivate at another rate, and process again.
        (*instance.plugin).stop_processing.unwrap()(instance.plugin);
        (*instance.plugin).deactivate.unwrap()(instance.plugin);
        assert!((*instance.plugin).activate.unwrap()(
            instance.plugin,
            44_100.0,
            1,
            512
        ));
        assert!(!(*instance.plugin).activate.unwrap()(
            instance.plugin,
            44_100.0,
            1,
            512
        ));
        assert!((*instance.plugin).start_processing.unwrap()(
            instance.plugin
        ));
        let [out, _] = instance.process(vec![note(CLAP_EVENT_NOTE_ON, 0, 60, 1.0)], None);
        assert!(peak(&out) > 0.01);
    });
}
