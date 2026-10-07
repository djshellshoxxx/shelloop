//! Raw CLAP ABI: entry point, factory, plugin vtable and extensions.
//!
//! Every callback is wrapped in [`guard`], which catches panics so that
//! unwinding never crosses the FFI boundary; a caught panic turns into the
//! callback's failure value. Threading follows the CLAP specification:
//!
//! - Parameter values live in atomics ([`ParamStore`]) and may be read from
//!   any thread.
//! - The audio state ([`Processor`]) sits in an `UnsafeCell`. It is created
//!   in `activate` and dropped in `deactivate` (main thread, never while
//!   processing) and otherwise only touched from `process`, `reset`,
//!   `start_processing` and `stop_processing` (audio thread). CLAP never runs
//!   these concurrently, which is what makes the unsynchronised access sound.
//! - `params.flush` and `state.load` never touch the audio state; they write
//!   the atomics and raise `params_dirty`, which `process` picks up at the
//!   start of the next block.

use crate::params::{ParamId, ParamStore, PARAMS, PARAM_COUNT};
use crate::processor::{HostEvent, Processor};
use crate::sequencer::TransportInfo;
use crate::state;
use crate::{BUNDLED_PATTERN_JSON, PLUGIN_ID};
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    clap_event_header, clap_event_midi, clap_event_note, clap_event_param_value,
    clap_event_transport, clap_input_events, clap_output_events, CLAP_CORE_EVENT_SPACE_ID,
    CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_CHOKE, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON,
    CLAP_EVENT_PARAM_VALUE, CLAP_EVENT_TRANSPORT, CLAP_TRANSPORT_HAS_BEATS_TIMELINE,
    CLAP_TRANSPORT_HAS_TEMPO, CLAP_TRANSPORT_IS_PLAYING,
};
use clap_sys::ext::audio_ports::{
    clap_audio_port_info, clap_plugin_audio_ports, CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS,
    CLAP_PORT_STEREO,
};
use clap_sys::ext::latency::{clap_plugin_latency, CLAP_EXT_LATENCY};
use clap_sys::ext::note_ports::{
    clap_note_port_info, clap_plugin_note_ports, CLAP_EXT_NOTE_PORTS, CLAP_NOTE_DIALECT_CLAP,
    CLAP_NOTE_DIALECT_MIDI,
};
use clap_sys::ext::params::{
    clap_host_params, clap_param_info, clap_plugin_params, CLAP_EXT_PARAMS,
    CLAP_PARAM_IS_AUTOMATABLE, CLAP_PARAM_IS_ENUM, CLAP_PARAM_IS_STEPPED, CLAP_PARAM_RESCAN_VALUES,
};
use clap_sys::ext::state::{clap_plugin_state, CLAP_EXT_STATE};
use clap_sys::factory::plugin_factory::{clap_plugin_factory, CLAP_PLUGIN_FACTORY_ID};
use clap_sys::fixedpoint::CLAP_BEATTIME_FACTOR;
use clap_sys::host::clap_host;
use clap_sys::id::{clap_id, CLAP_INVALID_ID};
use clap_sys::plugin::{clap_plugin, clap_plugin_descriptor};
use clap_sys::plugin_features::{
    CLAP_PLUGIN_FEATURE_INSTRUMENT, CLAP_PLUGIN_FEATURE_STEREO, CLAP_PLUGIN_FEATURE_SYNTHESIZER,
};
use clap_sys::process::{
    clap_process, clap_process_status, CLAP_PROCESS_CONTINUE, CLAP_PROCESS_ERROR,
};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::version::{clap_version_is_compatible, CLAP_VERSION};
use shelloop::{parse_pattern_json, Pattern};
use std::cell::UnsafeCell;
use std::ffi::{c_char, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::OnceLock;

/// Run `body`, converting a panic into `fallback` so it never unwinds into
/// the host.
fn guard<T>(fallback: T, body: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(fallback)
}

// ---------------------------------------------------------------------------
// Descriptor, factory and entry
// ---------------------------------------------------------------------------

struct Features([*const c_char; 4]);

// SAFETY: the array only holds pointers to immutable 'static C strings (and a
// terminating null), so sharing it between threads is sound.
unsafe impl Sync for Features {}

static FEATURES: Features = Features([
    CLAP_PLUGIN_FEATURE_INSTRUMENT.as_ptr(),
    CLAP_PLUGIN_FEATURE_SYNTHESIZER.as_ptr(),
    CLAP_PLUGIN_FEATURE_STEREO.as_ptr(),
    null(),
]);

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");

static DESCRIPTOR: clap_plugin_descriptor = clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: c"com.circuitdriftlabs.shelloop".as_ptr(),
    name: c"SHELLOOP".as_ptr(),
    vendor: c"Circuit Drift Labs".as_ptr(),
    url: c"https://github.com/djshellshoxxx/shelloop".as_ptr(),
    manual_url: c"https://github.com/djshellshoxxx/shelloop".as_ptr(),
    support_url: c"https://github.com/djshellshoxxx/shelloop/issues".as_ptr(),
    version: VERSION.as_ptr().cast::<c_char>(),
    description: c"Polyphonic SHELLOOP synthesizer with a host-synced step sequencer".as_ptr(),
    features: FEATURES.0.as_ptr(),
};

/// The CLAP entry point looked up by hosts.
#[no_mangle]
#[allow(non_upper_case_globals)]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry {
    clap_version: CLAP_VERSION,
    init: Some(entry_init),
    deinit: Some(entry_deinit),
    get_factory: Some(entry_get_factory),
};

static FACTORY: clap_plugin_factory = clap_plugin_factory {
    get_plugin_count: Some(factory_get_plugin_count),
    get_plugin_descriptor: Some(factory_get_plugin_descriptor),
    create_plugin: Some(factory_create_plugin),
};

unsafe extern "C" fn entry_init(_plugin_path: *const c_char) -> bool {
    // Nothing global to set up: the pattern is parsed per instance in `init`.
    true
}

unsafe extern "C" fn entry_deinit() {}

unsafe extern "C" fn entry_get_factory(factory_id: *const c_char) -> *const c_void {
    guard(null(), || {
        if factory_id.is_null() {
            return null();
        }
        // SAFETY: the host passes a valid NUL-terminated string (checked non-null).
        let id = unsafe { CStr::from_ptr(factory_id) };
        if id == CLAP_PLUGIN_FACTORY_ID {
            (&FACTORY as *const clap_plugin_factory).cast()
        } else {
            null()
        }
    })
}

unsafe extern "C" fn factory_get_plugin_count(_factory: *const clap_plugin_factory) -> u32 {
    1
}

unsafe extern "C" fn factory_get_plugin_descriptor(
    _factory: *const clap_plugin_factory,
    index: u32,
) -> *const clap_plugin_descriptor {
    if index == 0 {
        &DESCRIPTOR
    } else {
        null()
    }
}

unsafe extern "C" fn factory_create_plugin(
    _factory: *const clap_plugin_factory,
    host: *const clap_host,
    plugin_id: *const c_char,
) -> *const clap_plugin {
    guard(null(), || {
        if host.is_null() || plugin_id.is_null() {
            return null();
        }
        // SAFETY: both pointers are non-null and provided by the host per the
        // CLAP contract: `plugin_id` is NUL-terminated and `host` is a valid
        // clap_host that outlives the plugin.
        let (id, host_version) = unsafe { (CStr::from_ptr(plugin_id), (*host).clap_version) };
        if id.to_bytes() != PLUGIN_ID.as_bytes() || !clap_version_is_compatible(host_version) {
            return null();
        }
        let raw = Box::into_raw(Box::new(Plugin::new(host)));
        // SAFETY: `raw` was just produced by Box::into_raw, so it is valid and
        // uniquely owned here. The clap_plugin lives inside the boxed Plugin,
        // whose address stays stable until `destroy`.
        unsafe {
            (*raw).clap.plugin_data = raw.cast::<c_void>();
            &(*raw).clap
        }
    })
}

// ---------------------------------------------------------------------------
// Plugin instance
// ---------------------------------------------------------------------------

struct Plugin {
    clap: clap_plugin,
    host: *const clap_host,
    host_params: AtomicPtr<clap_host_params>,
    params: ParamStore,
    params_dirty: AtomicBool,
    pattern: OnceLock<Pattern>,
    /// Audio-thread state; see the module docs for the access rules.
    processor: UnsafeCell<Option<Processor>>,
}

impl Plugin {
    fn new(host: *const clap_host) -> Self {
        Self {
            clap: clap_plugin {
                desc: &DESCRIPTOR,
                plugin_data: null_mut(),
                init: Some(plugin_init),
                destroy: Some(plugin_destroy),
                activate: Some(plugin_activate),
                deactivate: Some(plugin_deactivate),
                start_processing: Some(plugin_start_processing),
                stop_processing: Some(plugin_stop_processing),
                reset: Some(plugin_reset),
                process: Some(plugin_process),
                get_extension: Some(plugin_get_extension),
                on_main_thread: Some(plugin_on_main_thread),
            },
            host,
            host_params: AtomicPtr::new(null_mut()),
            params: ParamStore::default(),
            params_dirty: AtomicBool::new(false),
            pattern: OnceLock::new(),
            processor: UnsafeCell::new(None),
        }
    }

    /// Exclusive access to the audio state.
    ///
    /// # Safety
    /// The caller must be in a CLAP callback that has exclusive access to the
    /// audio state: `activate`/`deactivate` on the main thread, or
    /// `process`/`reset`/`start_processing`/`stop_processing` on the audio
    /// thread. CLAP guarantees these never overlap.
    #[allow(clippy::mut_from_ref)]
    unsafe fn processor(&self) -> &mut Option<Processor> {
        // SAFETY: exclusivity is guaranteed by the caller (see above).
        unsafe { &mut *self.processor.get() }
    }

    fn notify_host_param_values(&self) {
        let host_params = self.host_params.load(Ordering::Acquire);
        if host_params.is_null() || self.host.is_null() {
            return;
        }
        // SAFETY: `host_params` came from the host's get_extension during
        // `init` and stays valid for the plugin's lifetime; this runs on the
        // main thread as `rescan` requires.
        unsafe {
            if let Some(rescan) = (*host_params).rescan {
                rescan(self.host, CLAP_PARAM_RESCAN_VALUES);
            }
        }
    }
}

/// Recover the `Plugin` behind a `clap_plugin` pointer.
///
/// # Safety
/// `plugin` must be null or a pointer returned by `factory_create_plugin`
/// that has not been destroyed.
unsafe fn plugin_from<'a>(plugin: *const clap_plugin) -> Option<&'a Plugin> {
    if plugin.is_null() {
        return None;
    }
    // SAFETY: per the contract above, `plugin` points into a live Plugin and
    // `plugin_data` points at that Plugin.
    unsafe { (*plugin).plugin_data.cast::<Plugin>().cast_const().as_ref() }
}

unsafe extern "C" fn plugin_init(plugin: *const clap_plugin) -> bool {
    guard(false, || {
        // SAFETY: the host passes the pointer it got from create_plugin.
        let Some(plugin) = (unsafe { plugin_from(plugin) }) else {
            return false;
        };
        let Ok(pattern) = parse_pattern_json(BUNDLED_PATTERN_JSON) else {
            return false;
        };
        let _ = plugin.pattern.set(pattern);
        if !plugin.host.is_null() {
            // SAFETY: `host` is the valid clap_host given to create_plugin;
            // querying host extensions is allowed from `init`.
            let ext = unsafe {
                (*plugin.host)
                    .get_extension
                    .map_or(null(), |get| get(plugin.host, CLAP_EXT_PARAMS.as_ptr()))
            };
            plugin
                .host_params
                .store(ext.cast::<clap_host_params>().cast_mut(), Ordering::Release);
        }
        true
    })
}

unsafe extern "C" fn plugin_destroy(plugin: *const clap_plugin) {
    guard((), || {
        if plugin.is_null() {
            return;
        }
        // SAFETY: `plugin_data` is the pointer from Box::into_raw in
        // create_plugin; destroy is called exactly once, after which the host
        // never uses the instance again.
        unsafe {
            let data = (*plugin).plugin_data.cast::<Plugin>();
            if !data.is_null() {
                drop(Box::from_raw(data));
            }
        }
    });
}

unsafe extern "C" fn plugin_activate(
    plugin: *const clap_plugin,
    sample_rate: f64,
    _min_frames: u32,
    _max_frames: u32,
) -> bool {
    guard(false, || {
        // SAFETY: host-provided instance pointer.
        let Some(plugin) = (unsafe { plugin_from(plugin) }) else {
            return false;
        };
        let Some(pattern) = plugin.pattern.get() else {
            return false;
        };
        // SAFETY: activate runs on the main thread while not processing.
        let slot = unsafe { plugin.processor() };
        if slot.is_some() {
            return false;
        }
        plugin.params_dirty.store(false, Ordering::Release);
        match Processor::new(sample_rate, plugin.params.snapshot(), pattern) {
            Ok(processor) => {
                *slot = Some(processor);
                true
            }
            Err(_) => false,
        }
    })
}

unsafe extern "C" fn plugin_deactivate(plugin: *const clap_plugin) {
    guard((), || {
        // SAFETY: host-provided instance pointer.
        if let Some(plugin) = unsafe { plugin_from(plugin) } {
            // SAFETY: deactivate runs on the main thread while not processing.
            unsafe { *plugin.processor() = None };
        }
    });
}

unsafe extern "C" fn plugin_start_processing(plugin: *const clap_plugin) -> bool {
    // SAFETY: host-provided instance pointer.
    guard(false, || unsafe { plugin_from(plugin) }.is_some())
}

unsafe extern "C" fn plugin_stop_processing(plugin: *const clap_plugin) {
    guard((), || {
        // SAFETY: host-provided instance pointer.
        if let Some(plugin) = unsafe { plugin_from(plugin) } {
            // SAFETY: stop_processing runs on the audio thread, exclusive with process.
            if let Some(processor) = unsafe { plugin.processor() } {
                processor.release_sequencer();
            }
        }
    });
}

unsafe extern "C" fn plugin_reset(plugin: *const clap_plugin) {
    guard((), || {
        // SAFETY: host-provided instance pointer.
        if let Some(plugin) = unsafe { plugin_from(plugin) } {
            // SAFETY: reset runs on the audio thread, exclusive with process.
            if let Some(processor) = unsafe { plugin.processor() } {
                processor.panic();
            }
        }
    });
}

unsafe extern "C" fn plugin_on_main_thread(_plugin: *const clap_plugin) {}

unsafe extern "C" fn plugin_get_extension(
    _plugin: *const clap_plugin,
    id: *const c_char,
) -> *const c_void {
    guard(null(), || {
        if id.is_null() {
            return null();
        }
        // SAFETY: the host passes a valid NUL-terminated extension id.
        let id = unsafe { CStr::from_ptr(id) };
        if id == CLAP_EXT_PARAMS {
            (&PARAMS_EXT as *const clap_plugin_params).cast()
        } else if id == CLAP_EXT_NOTE_PORTS {
            (&NOTE_PORTS_EXT as *const clap_plugin_note_ports).cast()
        } else if id == CLAP_EXT_AUDIO_PORTS {
            (&AUDIO_PORTS_EXT as *const clap_plugin_audio_ports).cast()
        } else if id == CLAP_EXT_STATE {
            (&STATE_EXT as *const clap_plugin_state).cast()
        } else if id == CLAP_EXT_LATENCY {
            (&LATENCY_EXT as *const clap_plugin_latency).cast()
        } else {
            null()
        }
    })
}

// ---------------------------------------------------------------------------
// Processing
// ---------------------------------------------------------------------------

/// Read the transport, returning `None` unless the host is playing and
/// provides both tempo and a beat timeline.
///
/// # Safety
/// `transport` must be null or point to a valid `clap_event_transport`.
unsafe fn read_transport(transport: *const clap_event_transport) -> Option<TransportInfo> {
    // SAFETY: guaranteed by the caller.
    let transport = unsafe { transport.as_ref() }?;
    let required =
        CLAP_TRANSPORT_IS_PLAYING | CLAP_TRANSPORT_HAS_TEMPO | CLAP_TRANSPORT_HAS_BEATS_TIMELINE;
    if transport.flags & required != required {
        return None;
    }
    Some(TransportInfo {
        playing: true,
        song_pos_beats: transport.song_pos_beats as f64 / CLAP_BEATTIME_FACTOR as f64,
        tempo: transport.tempo,
    })
}

/// Decode one core event into a [`HostEvent`].
///
/// # Safety
/// `header` must be null or point to a valid event whose `size` covers the
/// whole event, as CLAP requires.
unsafe fn decode_event(header: *const clap_event_header) -> Option<HostEvent> {
    // SAFETY: guaranteed by the caller.
    let event = unsafe { header.as_ref() }?;
    if event.space_id != CLAP_CORE_EVENT_SPACE_ID {
        return None;
    }
    let size = event.size as usize;
    // SAFETY (all casts below): the event type selects the concrete struct
    // per the CLAP ABI, and `size` is checked to cover it before reading.
    match event.type_ {
        CLAP_EVENT_NOTE_ON | CLAP_EVENT_NOTE_OFF | CLAP_EVENT_NOTE_CHOKE
            if size >= size_of::<clap_event_note>() =>
        {
            let note = unsafe { &*header.cast::<clap_event_note>() };
            Some(match event.type_ {
                CLAP_EVENT_NOTE_ON => HostEvent::NoteOn {
                    channel: note.channel,
                    key: note.key,
                    velocity: note.velocity,
                },
                CLAP_EVENT_NOTE_OFF => HostEvent::NoteOff {
                    channel: note.channel,
                    key: note.key,
                },
                _ => HostEvent::Choke {
                    channel: note.channel,
                    key: note.key,
                },
            })
        }
        CLAP_EVENT_MIDI if size >= size_of::<clap_event_midi>() => {
            let midi = unsafe { &*header.cast::<clap_event_midi>() };
            Some(HostEvent::Midi(midi.data))
        }
        CLAP_EVENT_PARAM_VALUE if size >= size_of::<clap_event_param_value>() => {
            let param = unsafe { &*header.cast::<clap_event_param_value>() };
            Some(HostEvent::ParamValue {
                id: param.param_id,
                value: param.value,
            })
        }
        CLAP_EVENT_TRANSPORT if size >= size_of::<clap_event_transport>() => {
            let transport = header.cast::<clap_event_transport>();
            Some(HostEvent::Transport(unsafe { read_transport(transport) }))
        }
        _ => None,
    }
}

/// Safe view over a host input event list.
struct InputEvents(*const clap_input_events);

impl InputEvents {
    fn len(&self) -> u32 {
        // SAFETY: the list pointer comes from the host and is valid for the
        // duration of the callback (null is handled).
        unsafe {
            self.0
                .as_ref()
                .and_then(|list| list.size.map(|size| size(self.0)))
                .unwrap_or(0)
        }
    }

    fn get(&self, index: u32) -> *const clap_event_header {
        // SAFETY: as in `len`; `index` is below `len()` at every call site.
        unsafe {
            self.0
                .as_ref()
                .and_then(|list| list.get.map(|get| get(self.0, index)))
                .unwrap_or(null())
        }
    }

    fn time(&self, index: u32) -> Option<u32> {
        // SAFETY: event pointers returned by the host are valid or null.
        unsafe { self.get(index).as_ref() }.map(|header| header.time)
    }

    fn decode(&self, index: u32) -> Option<HostEvent> {
        // SAFETY: event pointers returned by the host are valid or null.
        unsafe { decode_event(self.get(index)) }
    }
}

/// The stereo output channels of one process call.
struct Output {
    left: *mut f32,
    right: *mut f32,
}

impl Output {
    /// # Safety
    /// `process` must be the host's valid process struct for this call.
    unsafe fn from_process(process: &clap_process) -> Option<Self> {
        if process.audio_outputs_count == 0 || process.audio_outputs.is_null() {
            return None;
        }
        // SAFETY: audio_outputs is non-null with at least one buffer.
        let buffer: &mut clap_audio_buffer = unsafe { &mut *process.audio_outputs };
        if buffer.data32.is_null() || buffer.channel_count == 0 {
            return None;
        }
        buffer.constant_mask = 0;
        // SAFETY: data32 holds `channel_count` channel pointers.
        let left = unsafe { *buffer.data32 };
        let right = if buffer.channel_count >= 2 {
            // SAFETY: as above, index 1 < channel_count.
            unsafe { *buffer.data32.add(1) }
        } else {
            left
        };
        if left.is_null() || right.is_null() {
            return None;
        }
        Some(Self { left, right })
    }

    /// # Safety
    /// `index` must be below the block's `frames_count`.
    unsafe fn write(&self, index: usize, sample: f32) {
        // SAFETY: each channel holds `frames_count` samples (caller bound).
        unsafe {
            self.left.add(index).write(sample);
            if self.right != self.left {
                self.right.add(index).write(sample);
            }
        }
    }
}

unsafe extern "C" fn plugin_process(
    plugin: *const clap_plugin,
    process: *const clap_process,
) -> clap_process_status {
    guard(CLAP_PROCESS_ERROR, || {
        // SAFETY: host-provided instance and process pointers, valid for the call.
        let (Some(plugin), Some(process)) =
            (unsafe { plugin_from(plugin) }, unsafe { process.as_ref() })
        else {
            return CLAP_PROCESS_ERROR;
        };
        // SAFETY: process runs on the audio thread with exclusive access.
        let Some(processor) = (unsafe { plugin.processor() }).as_mut() else {
            return CLAP_PROCESS_ERROR;
        };
        if plugin.params_dirty.swap(false, Ordering::AcqRel) {
            processor.sync_params(plugin.params.snapshot());
        }
        // SAFETY: the transport pointer is null or valid for this call.
        let transport = unsafe { read_transport(process.transport) };
        processor.handle(HostEvent::Transport(transport), &plugin.params);

        let frames = process.frames_count as usize;
        let events = InputEvents(process.in_events);
        let count = events.len();
        let mut next = 0u32;

        let output = if frames > 0 {
            // SAFETY: `process` is the host's valid process struct.
            match unsafe { Output::from_process(process) } {
                Some(output) => Some(output),
                None => return CLAP_PROCESS_ERROR,
            }
        } else {
            None
        };

        let mut frame = 0usize;
        while frame < frames {
            // Apply every event due at or before this frame.
            while next < count && events.time(next).is_none_or(|t| t as usize <= frame) {
                if let Some(event) = events.decode(next) {
                    processor.handle(event, &plugin.params);
                }
                next += 1;
            }
            let until = if next < count {
                events
                    .time(next)
                    .map_or(frames, |t| (t as usize).min(frames))
            } else {
                frames
            };
            if let Some(output) = &output {
                // SAFETY: render only yields indices in frame..until <= frames.
                processor.render(frame..until, |index, sample| unsafe {
                    output.write(index, sample)
                });
            }
            frame = until;
        }
        // Events stamped at or past the block end still take effect.
        while next < count {
            if let Some(event) = events.decode(next) {
                processor.handle(event, &plugin.params);
            }
            next += 1;
        }
        CLAP_PROCESS_CONTINUE
    })
}

// ---------------------------------------------------------------------------
// Extensions
// ---------------------------------------------------------------------------

/// Copy `text` into a C buffer of `capacity` bytes, truncating and always
/// NUL-terminating.
fn write_c_string(buffer: &mut [c_char], text: &str) {
    let Some(last) = buffer.len().checked_sub(1) else {
        return;
    };
    let len = text.len().min(last);
    for (dst, src) in buffer.iter_mut().zip(&text.as_bytes()[..len]) {
        *dst = *src as c_char;
    }
    buffer[len] = 0;
}

static PARAMS_EXT: clap_plugin_params = clap_plugin_params {
    count: Some(params_count),
    get_info: Some(params_get_info),
    get_value: Some(params_get_value),
    value_to_text: Some(params_value_to_text),
    text_to_value: Some(params_text_to_value),
    flush: Some(params_flush),
};

unsafe extern "C" fn params_count(_plugin: *const clap_plugin) -> u32 {
    PARAM_COUNT as u32
}

unsafe extern "C" fn params_get_info(
    _plugin: *const clap_plugin,
    index: u32,
    info: *mut clap_param_info,
) -> bool {
    guard(false, || {
        let Some(spec) = PARAMS.get(index as usize) else {
            return false;
        };
        // SAFETY: the host passes a writable clap_param_info (null checked).
        let Some(info) = (unsafe { info.as_mut() }) else {
            return false;
        };
        let mut flags = CLAP_PARAM_IS_AUTOMATABLE;
        if spec.is_stepped() {
            flags |= CLAP_PARAM_IS_STEPPED;
        }
        if matches!(spec.kind, crate::params::ParamKind::Enum(_)) {
            flags |= CLAP_PARAM_IS_ENUM;
        }
        info.id = spec.id.raw();
        info.flags = flags;
        info.cookie = null_mut();
        write_c_string(&mut info.name, spec.name);
        write_c_string(&mut info.module, spec.module);
        info.min_value = spec.min;
        info.max_value = spec.max;
        info.default_value = spec.default;
        true
    })
}

unsafe extern "C" fn params_get_value(
    plugin: *const clap_plugin,
    param_id: clap_id,
    out_value: *mut f64,
) -> bool {
    guard(false, || {
        // SAFETY: host-provided instance pointer.
        let Some(plugin) = (unsafe { plugin_from(plugin) }) else {
            return false;
        };
        let (Some(id), false) = (ParamId::from_raw(param_id), out_value.is_null()) else {
            return false;
        };
        // SAFETY: out_value is non-null and writable per the CLAP contract.
        unsafe { out_value.write(plugin.params.get(id)) };
        true
    })
}

unsafe extern "C" fn params_value_to_text(
    _plugin: *const clap_plugin,
    param_id: clap_id,
    value: f64,
    out_buffer: *mut c_char,
    out_buffer_capacity: u32,
) -> bool {
    guard(false, || {
        let Some(id) = ParamId::from_raw(param_id) else {
            return false;
        };
        if out_buffer.is_null() || out_buffer_capacity == 0 || !value.is_finite() {
            return false;
        }
        // SAFETY: the host provides a writable buffer of the given capacity.
        let buffer =
            unsafe { std::slice::from_raw_parts_mut(out_buffer, out_buffer_capacity as usize) };
        write_c_string(buffer, &id.spec().format(value));
        true
    })
}

unsafe extern "C" fn params_text_to_value(
    _plugin: *const clap_plugin,
    param_id: clap_id,
    text: *const c_char,
    out_value: *mut f64,
) -> bool {
    guard(false, || {
        let Some(id) = ParamId::from_raw(param_id) else {
            return false;
        };
        if text.is_null() || out_value.is_null() {
            return false;
        }
        // SAFETY: the host passes a valid NUL-terminated string.
        let Ok(text) = (unsafe { CStr::from_ptr(text) }).to_str() else {
            return false;
        };
        match id.spec().parse(text) {
            Some(value) => {
                // SAFETY: out_value is non-null and writable.
                unsafe { out_value.write(value) };
                true
            }
            None => false,
        }
    })
}

unsafe extern "C" fn params_flush(
    plugin: *const clap_plugin,
    in_events: *const clap_input_events,
    _out_events: *const clap_output_events,
) {
    guard((), || {
        // SAFETY: host-provided instance pointer.
        let Some(plugin) = (unsafe { plugin_from(plugin) }) else {
            return;
        };
        let events = InputEvents(in_events);
        let mut changed = false;
        for index in 0..events.len() {
            if let Some(HostEvent::ParamValue { id, value }) = events.decode(index) {
                let Some(id) = ParamId::from_raw(id) else {
                    continue;
                };
                if let Some(value) = id.spec().sanitize(value) {
                    plugin.params.set(id, value);
                    changed = true;
                }
            }
        }
        if changed {
            plugin.params_dirty.store(true, Ordering::Release);
        }
    });
}

static NOTE_PORTS_EXT: clap_plugin_note_ports = clap_plugin_note_ports {
    count: Some(note_ports_count),
    get: Some(note_ports_get),
};

unsafe extern "C" fn note_ports_count(_plugin: *const clap_plugin, is_input: bool) -> u32 {
    u32::from(is_input)
}

unsafe extern "C" fn note_ports_get(
    _plugin: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_note_port_info,
) -> bool {
    guard(false, || {
        if !is_input || index != 0 {
            return false;
        }
        // SAFETY: the host passes a writable clap_note_port_info (null checked).
        let Some(info) = (unsafe { info.as_mut() }) else {
            return false;
        };
        info.id = 0;
        info.supported_dialects = CLAP_NOTE_DIALECT_CLAP | CLAP_NOTE_DIALECT_MIDI;
        info.preferred_dialect = CLAP_NOTE_DIALECT_CLAP;
        write_c_string(&mut info.name, "Notes");
        true
    })
}

static AUDIO_PORTS_EXT: clap_plugin_audio_ports = clap_plugin_audio_ports {
    count: Some(audio_ports_count),
    get: Some(audio_ports_get),
};

unsafe extern "C" fn audio_ports_count(_plugin: *const clap_plugin, is_input: bool) -> u32 {
    u32::from(!is_input)
}

unsafe extern "C" fn audio_ports_get(
    _plugin: *const clap_plugin,
    index: u32,
    is_input: bool,
    info: *mut clap_audio_port_info,
) -> bool {
    guard(false, || {
        if is_input || index != 0 {
            return false;
        }
        // SAFETY: the host passes a writable clap_audio_port_info (null checked).
        let Some(info) = (unsafe { info.as_mut() }) else {
            return false;
        };
        info.id = 0;
        write_c_string(&mut info.name, "Output");
        // 32-bit float only: CLAP_AUDIO_PORT_SUPPORTS_64BITS is not set.
        info.flags = CLAP_AUDIO_PORT_IS_MAIN;
        info.channel_count = 2;
        info.port_type = CLAP_PORT_STEREO.as_ptr();
        info.in_place_pair = CLAP_INVALID_ID;
        true
    })
}

static STATE_EXT: clap_plugin_state = clap_plugin_state {
    save: Some(state_save),
    load: Some(state_load),
};

unsafe extern "C" fn state_save(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool {
    guard(false, || {
        // SAFETY: host-provided instance pointer.
        let Some(plugin) = (unsafe { plugin_from(plugin) }) else {
            return false;
        };
        // SAFETY: the host passes a valid output stream for this call.
        let Some(write) = (unsafe { stream.as_ref() }).and_then(|stream| stream.write) else {
            return false;
        };
        let bytes = state::encode(&plugin.params.snapshot());
        let mut written = 0usize;
        while written < bytes.len() {
            let remaining = &bytes[written..];
            // SAFETY: `remaining` is a live buffer of the given length.
            let result = unsafe {
                write(
                    stream,
                    remaining.as_ptr().cast::<c_void>(),
                    remaining.len() as u64,
                )
            };
            // A stream may accept fewer bytes than offered; 0 or negative
            // means it cannot make progress.
            let Ok(count) = usize::try_from(result) else {
                return false;
            };
            if count == 0 || count > remaining.len() {
                return false;
            }
            written += count;
        }
        true
    })
}

unsafe extern "C" fn state_load(plugin: *const clap_plugin, stream: *const clap_istream) -> bool {
    guard(false, || {
        // SAFETY: host-provided instance pointer.
        let Some(plugin) = (unsafe { plugin_from(plugin) }) else {
            return false;
        };
        // SAFETY: the host passes a valid input stream for this call.
        let Some(read) = (unsafe { stream.as_ref() }).and_then(|stream| stream.read) else {
            return false;
        };
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            // SAFETY: `chunk` is a writable buffer of the given length.
            let result = unsafe {
                read(
                    stream,
                    chunk.as_mut_ptr().cast::<c_void>(),
                    chunk.len() as u64,
                )
            };
            // Negative is an error, 0 is end of stream, otherwise a
            // (possibly partial) chunk.
            let Ok(count) = usize::try_from(result) else {
                return false;
            };
            if count == 0 {
                break;
            }
            let Some(data) = chunk.get(..count) else {
                return false;
            };
            bytes.extend_from_slice(data);
            if bytes.len() > state::MAX_STATE_BYTES {
                return false;
            }
        }
        let Ok(values) = state::decode(&bytes) else {
            return false;
        };
        plugin.params.store_all(&values);
        plugin.params_dirty.store(true, Ordering::Release);
        plugin.notify_host_param_values();
        true
    })
}

static LATENCY_EXT: clap_plugin_latency = clap_plugin_latency {
    get: Some(latency_get),
};

unsafe extern "C" fn latency_get(_plugin: *const clap_plugin) -> u32 {
    0
}
