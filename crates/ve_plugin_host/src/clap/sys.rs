//! The parts of the CLAP 1.x C ABI the host uses, transcribed from
//! `third_party/clap/include/clap` (entry, factory, plugin, host, process,
//! audio buffers, events, params, audio-ports, state, thread-check, log).
//! Layouts are `#[repr(C)]` copies; `tests/clap_abi.rs` pins their sizes.
//!
//! The test plugin (`crates/clap_test_gain`) includes this file too.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_void};

pub type clap_id = u32;
pub const CLAP_INVALID_ID: clap_id = u32::MAX;
pub const CLAP_NAME_SIZE: usize = 256;
pub const CLAP_PATH_SIZE: usize = 1024;

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct clap_version {
    pub major: u32,
    pub minor: u32,
    pub revision: u32,
}

pub const CLAP_VERSION: clap_version = clap_version { major: 1, minor: 2, revision: 10 };

#[repr(C)]
pub struct clap_plugin_entry {
    pub clap_version: clap_version,
    pub init: unsafe extern "C" fn(plugin_path: *const c_char) -> bool,
    pub deinit: unsafe extern "C" fn(),
    pub get_factory: unsafe extern "C" fn(factory_id: *const c_char) -> *const c_void,
}

pub const CLAP_PLUGIN_FACTORY_ID: &[u8] = b"clap.plugin-factory\0";

#[repr(C)]
pub struct clap_plugin_factory {
    pub get_plugin_count: unsafe extern "C" fn(factory: *const clap_plugin_factory) -> u32,
    pub get_plugin_descriptor: unsafe extern "C" fn(factory: *const clap_plugin_factory, index: u32) -> *const clap_plugin_descriptor,
    pub create_plugin: unsafe extern "C" fn(factory: *const clap_plugin_factory, host: *const clap_host, plugin_id: *const c_char) -> *const clap_plugin,
}

#[repr(C)]
pub struct clap_plugin_descriptor {
    pub clap_version: clap_version,
    pub id: *const c_char,
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub url: *const c_char,
    pub manual_url: *const c_char,
    pub support_url: *const c_char,
    pub version: *const c_char,
    pub description: *const c_char,
    /// Null-terminated array of C strings.
    pub features: *const *const c_char,
}

#[repr(C)]
pub struct clap_plugin {
    pub desc: *const clap_plugin_descriptor,
    pub plugin_data: *mut c_void,
    pub init: unsafe extern "C" fn(plugin: *const clap_plugin) -> bool,
    pub destroy: unsafe extern "C" fn(plugin: *const clap_plugin),
    pub activate: unsafe extern "C" fn(plugin: *const clap_plugin, sample_rate: f64, min_frames: u32, max_frames: u32) -> bool,
    pub deactivate: unsafe extern "C" fn(plugin: *const clap_plugin),
    pub start_processing: unsafe extern "C" fn(plugin: *const clap_plugin) -> bool,
    pub stop_processing: unsafe extern "C" fn(plugin: *const clap_plugin),
    pub reset: unsafe extern "C" fn(plugin: *const clap_plugin),
    pub process: unsafe extern "C" fn(plugin: *const clap_plugin, process: *const clap_process) -> i32,
    pub get_extension: unsafe extern "C" fn(plugin: *const clap_plugin, id: *const c_char) -> *const c_void,
    pub on_main_thread: unsafe extern "C" fn(plugin: *const clap_plugin),
}

#[repr(C)]
pub struct clap_host {
    pub clap_version: clap_version,
    pub host_data: *mut c_void,
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub url: *const c_char,
    pub version: *const c_char,
    pub get_extension: unsafe extern "C" fn(host: *const clap_host, extension_id: *const c_char) -> *const c_void,
    pub request_restart: unsafe extern "C" fn(host: *const clap_host),
    pub request_process: unsafe extern "C" fn(host: *const clap_host),
    pub request_callback: unsafe extern "C" fn(host: *const clap_host),
}

pub const CLAP_PROCESS_ERROR: i32 = 0;
pub const CLAP_PROCESS_CONTINUE: i32 = 1;

#[repr(C)]
pub struct clap_process {
    pub steady_time: i64,
    pub frames_count: u32,
    pub transport: *const c_void,
    pub audio_inputs: *const clap_audio_buffer,
    pub audio_outputs: *mut clap_audio_buffer,
    pub audio_inputs_count: u32,
    pub audio_outputs_count: u32,
    pub in_events: *const clap_input_events,
    pub out_events: *const clap_output_events,
}

#[repr(C)]
pub struct clap_audio_buffer {
    pub data32: *mut *mut f32,
    pub data64: *mut *mut f64,
    pub channel_count: u32,
    pub latency: u32,
    pub constant_mask: u64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct clap_event_header {
    pub size: u32,
    pub time: u32,
    pub space_id: u16,
    pub type_: u16,
    pub flags: u32,
}

pub const CLAP_CORE_EVENT_SPACE_ID: u16 = 0;
pub const CLAP_EVENT_PARAM_VALUE: u16 = 5;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct clap_event_param_value {
    pub header: clap_event_header,
    pub param_id: clap_id,
    pub cookie: *mut c_void,
    pub note_id: i32,
    pub port_index: i16,
    pub channel: i16,
    pub key: i16,
    pub value: f64,
}

#[repr(C)]
pub struct clap_input_events {
    pub ctx: *mut c_void,
    pub size: unsafe extern "C" fn(list: *const clap_input_events) -> u32,
    pub get: unsafe extern "C" fn(list: *const clap_input_events, index: u32) -> *const clap_event_header,
}

#[repr(C)]
pub struct clap_output_events {
    pub ctx: *mut c_void,
    pub try_push: unsafe extern "C" fn(list: *const clap_output_events, event: *const clap_event_header) -> bool,
}

pub const CLAP_EXT_PARAMS: &[u8] = b"clap.params\0";
pub const CLAP_PARAM_IS_STEPPED: u32 = 1 << 0;
pub const CLAP_PARAM_IS_HIDDEN: u32 = 1 << 2;
pub const CLAP_PARAM_IS_READONLY: u32 = 1 << 3;
pub const CLAP_PARAM_IS_BYPASS: u32 = 1 << 4;
pub const CLAP_PARAM_IS_AUTOMATABLE: u32 = 1 << 5;
pub const CLAP_PARAM_IS_ENUM: u32 = 1 << 16;

#[repr(C)]
pub struct clap_param_info {
    pub id: clap_id,
    pub flags: u32,
    pub cookie: *mut c_void,
    pub name: [c_char; CLAP_NAME_SIZE],
    pub module: [c_char; CLAP_PATH_SIZE],
    pub min_value: f64,
    pub max_value: f64,
    pub default_value: f64,
}

#[repr(C)]
pub struct clap_plugin_params {
    pub count: unsafe extern "C" fn(plugin: *const clap_plugin) -> u32,
    pub get_info: unsafe extern "C" fn(plugin: *const clap_plugin, index: u32, info: *mut clap_param_info) -> bool,
    pub get_value: unsafe extern "C" fn(plugin: *const clap_plugin, id: clap_id, out: *mut f64) -> bool,
    pub value_to_text: unsafe extern "C" fn(plugin: *const clap_plugin, id: clap_id, value: f64, out: *mut c_char, cap: u32) -> bool,
    pub text_to_value: unsafe extern "C" fn(plugin: *const clap_plugin, id: clap_id, text: *const c_char, out: *mut f64) -> bool,
    pub flush: unsafe extern "C" fn(plugin: *const clap_plugin, input: *const clap_input_events, output: *const clap_output_events),
}

#[repr(C)]
pub struct clap_host_params {
    pub rescan: unsafe extern "C" fn(host: *const clap_host, flags: u32),
    pub clear: unsafe extern "C" fn(host: *const clap_host, id: clap_id, flags: u32),
    pub request_flush: unsafe extern "C" fn(host: *const clap_host),
}

pub const CLAP_EXT_AUDIO_PORTS: &[u8] = b"clap.audio-ports\0";
pub const CLAP_AUDIO_PORT_IS_MAIN: u32 = 1 << 0;

#[repr(C)]
pub struct clap_audio_port_info {
    pub id: clap_id,
    pub name: [c_char; CLAP_NAME_SIZE],
    pub flags: u32,
    pub channel_count: u32,
    pub port_type: *const c_char,
    pub in_place_pair: clap_id,
}

#[repr(C)]
pub struct clap_plugin_audio_ports {
    pub count: unsafe extern "C" fn(plugin: *const clap_plugin, is_input: bool) -> u32,
    pub get: unsafe extern "C" fn(plugin: *const clap_plugin, index: u32, is_input: bool, info: *mut clap_audio_port_info) -> bool,
}

pub const CLAP_EXT_THREAD_CHECK: &[u8] = b"clap.thread-check\0";

#[repr(C)]
pub struct clap_host_thread_check {
    pub is_main_thread: unsafe extern "C" fn(host: *const clap_host) -> bool,
    pub is_audio_thread: unsafe extern "C" fn(host: *const clap_host) -> bool,
}

pub const CLAP_EXT_LOG: &[u8] = b"clap.log\0";

#[repr(C)]
pub struct clap_host_log {
    pub log: unsafe extern "C" fn(host: *const clap_host, severity: i32, msg: *const c_char),
}

pub const CLAP_EXT_STATE: &[u8] = b"clap.state\0";

#[repr(C)]
pub struct clap_host_state {
    pub mark_dirty: unsafe extern "C" fn(host: *const clap_host),
}

#[repr(C)]
pub struct clap_istream {
    pub ctx: *mut c_void,
    pub read: unsafe extern "C" fn(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64,
}

#[repr(C)]
pub struct clap_ostream {
    pub ctx: *mut c_void,
    pub write: unsafe extern "C" fn(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64,
}

#[repr(C)]
pub struct clap_plugin_state {
    pub save: unsafe extern "C" fn(plugin: *const clap_plugin, stream: *const clap_ostream) -> bool,
    pub load: unsafe extern "C" fn(plugin: *const clap_plugin, stream: *const clap_istream) -> bool,
}

/// The feature a plugin lists to say it processes audio.
pub const FEATURE_AUDIO_EFFECT: &str = "audio-effect";
