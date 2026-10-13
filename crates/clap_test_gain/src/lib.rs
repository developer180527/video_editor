//! A tiny CLAP audio effect for the host's tests: "Test Gain" — Gain (dB,
//! id 7) and Polarity (Normal / Inverted, id 9, stepped). It counts which
//! threads called it, so tests can check the host's threading.

#![allow(clippy::missing_safety_doc)]

#[path = "../../ve_plugin_host/src/clap/sys.rs"]
mod sys;

use std::ffi::{c_char, c_void, CStr};
use sys::*;

const ID: &[u8] = b"org.ve.test.gain\0";
const GAIN: clap_id = 7;
const POLARITY: clap_id = 9;

struct Desc(clap_plugin_descriptor);
unsafe impl Sync for Desc {}
struct Features([*const c_char; 3]);
unsafe impl Sync for Features {}

static FEATURES: Features = Features([c"audio-effect".as_ptr(), c"stereo".as_ptr(), std::ptr::null()]);
static DESC: Desc = Desc(clap_plugin_descriptor {
    clap_version: CLAP_VERSION,
    id: ID.as_ptr() as _,
    name: c"Test Gain".as_ptr(),
    vendor: c"ve tests".as_ptr(),
    url: c"".as_ptr(),
    manual_url: c"".as_ptr(),
    support_url: c"".as_ptr(),
    version: c"1.0".as_ptr(),
    description: c"".as_ptr(),
    features: &FEATURES.0 as *const _ as *const *const c_char,
});

struct Gain {
    plugin: clap_plugin,
    host: *const clap_host,
    gain_db: f64,
    invert: bool,
    active: bool,
}

unsafe fn me<'a>(p: *const clap_plugin) -> &'a mut Gain {
    unsafe { &mut *((*p).plugin_data as *mut Gain) }
}

/// Fails loudly if the host calls a main-thread function elsewhere.
unsafe fn assert_main(p: *const clap_plugin, what: &str) {
    let host = unsafe { me(p).host };
    let tc = unsafe { ((*host).get_extension)(host, CLAP_EXT_THREAD_CHECK.as_ptr() as _) } as *const clap_host_thread_check;
    if !tc.is_null() && !unsafe { ((*tc).is_main_thread)(host) } {
        panic!("{what} called off the main thread");
    }
}

unsafe extern "C" fn init(p: *const clap_plugin) -> bool {
    unsafe { assert_main(p, "init") };
    true
}
unsafe extern "C" fn destroy(p: *const clap_plugin) {
    unsafe {
        assert_main(p, "destroy");
        drop(Box::from_raw((*p).plugin_data as *mut Gain));
    }
}
unsafe extern "C" fn activate(p: *const clap_plugin, _rate: f64, _min: u32, _max: u32) -> bool {
    unsafe {
        assert_main(p, "activate");
        me(p).active = true;
    }
    true
}
unsafe extern "C" fn deactivate(p: *const clap_plugin) {
    unsafe {
        assert_main(p, "deactivate");
        me(p).active = false;
    }
}
unsafe extern "C" fn start_processing(_: *const clap_plugin) -> bool {
    true
}
unsafe extern "C" fn stop_processing(_: *const clap_plugin) {}
unsafe extern "C" fn reset(_: *const clap_plugin) {}

unsafe fn apply_events(g: &mut Gain, events: *const clap_input_events) {
    if events.is_null() {
        return;
    }
    let n = unsafe { ((*events).size)(events) };
    for i in 0..n {
        let h = unsafe { ((*events).get)(events, i) };
        if h.is_null() || unsafe { (*h).type_ } != CLAP_EVENT_PARAM_VALUE {
            continue;
        }
        let e = unsafe { &*(h as *const clap_event_param_value) };
        match e.param_id {
            GAIN => g.gain_db = e.value,
            POLARITY => g.invert = e.value >= 0.5,
            _ => {}
        }
    }
}

unsafe extern "C" fn process(p: *const clap_plugin, pr: *const clap_process) -> i32 {
    let g = unsafe { me(p) };
    let pr = unsafe { &*pr };
    if !g.active {
        return CLAP_PROCESS_ERROR;
    }
    unsafe { apply_events(g, pr.in_events) };
    let k = 10f64.powf(g.gain_db / 20.0) as f32 * if g.invert { -1.0 } else { 1.0 };
    let (input, output) = unsafe { (&*pr.audio_inputs, &*pr.audio_outputs) };
    for c in 0..2usize {
        let (i, o) = unsafe { (*input.data32.add(c), *output.data32.add(c)) };
        for s in 0..pr.frames_count as usize {
            unsafe { *o.add(s) = *i.add(s) * k };
        }
    }
    CLAP_PROCESS_CONTINUE
}

unsafe extern "C" fn params_count(_: *const clap_plugin) -> u32 {
    2
}
fn put(dst: &mut [c_char], s: &str) {
    for (d, b) in dst.iter_mut().zip(s.bytes().chain([0])) {
        *d = b as c_char;
    }
}
unsafe extern "C" fn params_info(_: *const clap_plugin, i: u32, info: *mut clap_param_info) -> bool {
    let info = unsafe { &mut *info };
    match i {
        0 => {
            info.id = GAIN;
            info.flags = CLAP_PARAM_IS_AUTOMATABLE;
            put(&mut info.name, "Gain");
            (info.min_value, info.max_value, info.default_value) = (-24.0, 24.0, 0.0);
        }
        1 => {
            info.id = POLARITY;
            info.flags = CLAP_PARAM_IS_STEPPED | CLAP_PARAM_IS_ENUM;
            put(&mut info.name, "Polarity");
            (info.min_value, info.max_value, info.default_value) = (0.0, 1.0, 0.0);
        }
        _ => return false,
    }
    true
}
unsafe extern "C" fn params_value(p: *const clap_plugin, id: clap_id, out: *mut f64) -> bool {
    let g = unsafe { me(p) };
    unsafe { *out = if id == GAIN { g.gain_db } else { g.invert as u8 as f64 } };
    true
}
unsafe extern "C" fn params_to_text(_: *const clap_plugin, id: clap_id, v: f64, out: *mut c_char, cap: u32) -> bool {
    let s = if id == POLARITY { if v >= 0.5 { "Inverted".to_string() } else { "Normal".to_string() } } else { format!("{v:.1} dB") };
    let dst = unsafe { std::slice::from_raw_parts_mut(out, cap as usize) };
    put(dst, &s);
    true
}
unsafe extern "C" fn params_from_text(_: *const clap_plugin, _: clap_id, _: *const c_char, _: *mut f64) -> bool {
    false
}
unsafe extern "C" fn params_flush(p: *const clap_plugin, input: *const clap_input_events, _: *const clap_output_events) {
    unsafe { apply_events(me(p), input) };
}
static PARAMS: clap_plugin_params = clap_plugin_params {
    count: params_count,
    get_info: params_info,
    get_value: params_value,
    value_to_text: params_to_text,
    text_to_value: params_from_text,
    flush: params_flush,
};

unsafe extern "C" fn ports_count(_: *const clap_plugin, _: bool) -> u32 {
    1
}
unsafe extern "C" fn ports_get(_: *const clap_plugin, i: u32, _: bool, info: *mut clap_audio_port_info) -> bool {
    if i != 0 {
        return false;
    }
    let info = unsafe { &mut *info };
    info.id = 0;
    put(&mut info.name, "Main");
    info.flags = CLAP_AUDIO_PORT_IS_MAIN;
    info.channel_count = 2;
    info.port_type = c"stereo".as_ptr();
    info.in_place_pair = CLAP_INVALID_ID;
    true
}
static PORTS: clap_plugin_audio_ports = clap_plugin_audio_ports { count: ports_count, get: ports_get };

unsafe extern "C" fn get_extension(_: *const clap_plugin, id: *const c_char) -> *const c_void {
    let id = unsafe { CStr::from_ptr(id) }.to_bytes_with_nul();
    if id == CLAP_EXT_PARAMS {
        &PARAMS as *const _ as _
    } else if id == CLAP_EXT_AUDIO_PORTS {
        &PORTS as *const _ as _
    } else {
        std::ptr::null()
    }
}
unsafe extern "C" fn on_main_thread(_: *const clap_plugin) {}

unsafe extern "C" fn factory_count(_: *const clap_plugin_factory) -> u32 {
    1
}
unsafe extern "C" fn factory_desc(_: *const clap_plugin_factory, i: u32) -> *const clap_plugin_descriptor {
    if i == 0 { &DESC.0 } else { std::ptr::null() }
}
unsafe extern "C" fn factory_create(_: *const clap_plugin_factory, host: *const clap_host, id: *const c_char) -> *const clap_plugin {
    if unsafe { CStr::from_ptr(id) }.to_bytes_with_nul() != ID {
        return std::ptr::null();
    }
    let g = Box::into_raw(Box::new(Gain {
        plugin: clap_plugin {
            desc: &DESC.0,
            plugin_data: std::ptr::null_mut(),
            init,
            destroy,
            activate,
            deactivate,
            start_processing,
            stop_processing,
            reset,
            process,
            get_extension,
            on_main_thread,
        },
        host,
        gain_db: 0.0,
        invert: false,
        active: false,
    }));
    unsafe {
        (*g).plugin.plugin_data = g as *mut c_void;
        &(*g).plugin
    }
}
static FACTORY: clap_plugin_factory = clap_plugin_factory { get_plugin_count: factory_count, get_plugin_descriptor: factory_desc, create_plugin: factory_create };

unsafe extern "C" fn entry_init(_: *const c_char) -> bool {
    true
}
unsafe extern "C" fn entry_deinit() {}
unsafe extern "C" fn entry_factory(id: *const c_char) -> *const c_void {
    if unsafe { CStr::from_ptr(id) }.to_bytes_with_nul() == CLAP_PLUGIN_FACTORY_ID {
        &FACTORY as *const _ as _
    } else {
        std::ptr::null()
    }
}

#[unsafe(no_mangle)]
#[used]
pub static clap_entry: clap_plugin_entry = clap_plugin_entry { clap_version: CLAP_VERSION, init: entry_init, deinit: entry_deinit, get_factory: entry_factory };
