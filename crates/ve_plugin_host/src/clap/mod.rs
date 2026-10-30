//! CLAP audio plugin host (desktop: iPadOS cannot load plugin files, and
//! its `discover` finds none).
//!
//! - **Scan.** Each `.clap` file the platform finds is loaded once and kept
//!   loaded; every plugin in it that says `audio-effect` and has an audio
//!   input becomes an [`EffectInfo`] (`PluginApi::Clap`, its CLAP id), with
//!   its visible parameters as [`ParamInfo`]s keyed by the CLAP parameter
//!   id. Stepped parameters with a few values become choices named by the
//!   plugin's own text.
//! - **Threads.** CLAP's "main thread" is a thread of the host's own
//!   (`ve-clap`): plugins are created, initialised, activated, deactivated
//!   and destroyed there, and `on_main_thread` runs there when asked for.
//!   `process` runs on the mixer's thread, which is CLAP's audio thread.
//! - **Processing.** The mixer hands each instance a stereo block in place;
//!   parameter changes since the last block go in as `PARAM_VALUE` events at
//!   its start. Mono plugins get the sum and give it to both sides.
//!
//! Not yet: plugin editor windows (`clap.gui`) and saving a plugin's
//! private state (`clap.state`) in the project: parameters are the state.

pub mod sys;

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::ThreadId;

use sys::*;
use ve_model::{PluginApi, PluginRef};
use ve_ports::{AudioPlugins, AudioProcessor, NativeLibraries, NativeLibrary};

use crate::{EffectInfo, EffectKind, Implementation, ParamInfo, ParamKind};

pub fn discover(libs: &dyn NativeLibraries) -> Vec<String> {
    libs.discover("clap")
}

/// The most frames handed to a plugin at once (longer blocks are split).
const MAX_FRAMES: u32 = 4096;
type Job = Box<dyn FnOnce() + Send>;

/// The main thread as a plugin's host context sees it: which thread it is,
/// and how to wake it.
struct MainRef {
    id: ThreadId,
    kick: mpsc::Sender<Job>,
}

/// CLAP's main thread: runs jobs, and `on_main_thread` for instances that
/// asked.
struct MainThread {
    tx: Mutex<mpsc::Sender<Job>>,
    id: ThreadId,
    /// Instances that asked for `on_main_thread` (their host contexts).
    callbacks: Arc<Mutex<Vec<Arc<HostCtx>>>>,
}

impl MainThread {
    fn start() -> Arc<MainThread> {
        let (tx, rx) = mpsc::channel::<Job>();
        let callbacks: Arc<Mutex<Vec<Arc<HostCtx>>>> = Arc::default();
        let polled = callbacks.clone();
        let (id_tx, id_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("ve-clap".into())
            .spawn(move || {
                let _ = id_tx.send(std::thread::current().id());
                // Sleeps until there is work: a job, or a plugin's
                // `request_callback` (which posts an empty job to wake it).
                // No polling timer: idle means idle.
                while let Ok(job) = rx.recv() {
                    job();
                    let asked: Vec<Arc<HostCtx>> = polled.lock().unwrap().iter().filter(|c| c.wants_callback.swap(false, Ordering::SeqCst)).cloned().collect();
                    for ctx in asked {
                        let p = ctx.plugin.load(Ordering::SeqCst);
                        if !p.is_null() {
                            unsafe { ((*p).on_main_thread)(p) };
                        }
                    }
                }
            })
            .expect("CLAP main thread");
        let id = id_rx.recv().expect("CLAP main thread started");
        Arc::new(MainThread { tx: Mutex::new(tx), id, callbacks })
    }

    /// What a plugin's host context needs of the main thread.
    fn handle(&self) -> MainRef {
        MainRef { id: self.id, kick: self.tx.lock().unwrap().clone() }
    }

    /// Run `f` on the main thread and wait for its result.
    fn run<R: Send + 'static>(&self, f: impl FnOnce() -> R + Send + 'static) -> R {
        if std::thread::current().id() == self.id {
            return f();
        }
        let (tx, rx) = mpsc::channel();
        let job: Job = Box::new(move || {
            let _ = tx.send(f());
        });
        self.tx.lock().unwrap().send(job).expect("CLAP main thread alive");
        rx.recv().expect("CLAP main thread answered")
    }

    /// Run `f` on the main thread, without waiting.
    fn post(&self, f: impl FnOnce() + Send + 'static) {
        let _ = self.tx.lock().unwrap().send(Box::new(f));
    }
}

/// What a plugin instance sees as its host. Lives (boxed in an `Arc`, at a
/// fixed address) as long as the plugin does.
struct HostCtx {
    host: clap_host,
    main: ThreadId,
    wants_callback: AtomicBool,
    plugin: AtomicPtr<clap_plugin>,
    name: String,
    /// Wakes the main thread (an empty job) when the plugin asks for a callback.
    kick: Mutex<mpsc::Sender<Job>>,
}

// The raw pointers in `clap_host` are to static strings and to this context.
unsafe impl Send for HostCtx {}
unsafe impl Sync for HostCtx {}

static HOST_NAME: &[u8] = b"Video Editor\0";
static HOST_VENDOR: &[u8] = b"ve\0";
static HOST_URL: &[u8] = b"\0";
static HOST_VERSION: &[u8] = b"0.1\0";

unsafe fn ctx<'a>(host: *const clap_host) -> &'a HostCtx {
    unsafe { &*((*host).host_data as *const HostCtx) }
}

unsafe extern "C" fn is_main_thread(host: *const clap_host) -> bool {
    unsafe { std::thread::current().id() == ctx(host).main }
}
unsafe extern "C" fn is_audio_thread(host: *const clap_host) -> bool {
    unsafe { !is_main_thread(host) }
}
static THREAD_CHECK: clap_host_thread_check = clap_host_thread_check { is_main_thread, is_audio_thread };

unsafe extern "C" fn log(host: *const clap_host, severity: i32, msg: *const c_char) {
    if !msg.is_null() && severity >= 3 {
        let m = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
        eprintln!("CLAP {}: {m}", unsafe { &ctx(host).name });
    }
}
static LOG: clap_host_log = clap_host_log { log };

unsafe extern "C" fn params_rescan(_: *const clap_host, _: u32) {}
unsafe extern "C" fn params_clear(_: *const clap_host, _: clap_id, _: u32) {}
unsafe extern "C" fn params_flush(_: *const clap_host) {}
static HOST_PARAMS: clap_host_params = clap_host_params { rescan: params_rescan, clear: params_clear, request_flush: params_flush };

unsafe extern "C" fn mark_dirty(_: *const clap_host) {}
static HOST_STATE: clap_host_state = clap_host_state { mark_dirty };

unsafe extern "C" fn host_extension(_: *const clap_host, id: *const c_char) -> *const c_void {
    let id = unsafe { CStr::from_ptr(id) }.to_bytes_with_nul();
    if id == CLAP_EXT_THREAD_CHECK {
        &THREAD_CHECK as *const _ as *const c_void
    } else if id == CLAP_EXT_LOG {
        &LOG as *const _ as *const c_void
    } else if id == CLAP_EXT_PARAMS {
        &HOST_PARAMS as *const _ as *const c_void
    } else if id == CLAP_EXT_STATE {
        &HOST_STATE as *const _ as *const c_void
    } else {
        std::ptr::null()
    }
}
// A restart (latency or ports changed) or a process request needs nothing
// here: the mixer calls `process` on every block anyway.
unsafe extern "C" fn request_restart(_: *const clap_host) {}
unsafe extern "C" fn request_process(_: *const clap_host) {}
unsafe extern "C" fn request_callback(host: *const clap_host) {
    let c = unsafe { ctx(host) };
    c.wants_callback.store(true, Ordering::SeqCst);
    let _ = c.kick.lock().unwrap().send(Box::new(|| {}));
}

impl HostCtx {
    fn new(main: &MainRef, name: &str) -> Arc<HostCtx> {
        let ctx = Arc::new(HostCtx {
            host: clap_host {
                clap_version: CLAP_VERSION,
                host_data: std::ptr::null_mut(),
                name: HOST_NAME.as_ptr() as _,
                vendor: HOST_VENDOR.as_ptr() as _,
                url: HOST_URL.as_ptr() as _,
                version: HOST_VERSION.as_ptr() as _,
                get_extension: host_extension,
                request_restart,
                request_process,
                request_callback,
            },
            main: main.id,
            kick: Mutex::new(main.kick.clone()),
            wants_callback: AtomicBool::new(false),
            plugin: AtomicPtr::new(std::ptr::null_mut()),
            name: name.into(),
        });
        // The plugin finds its context through `host_data`.
        let p = Arc::as_ptr(&ctx);
        unsafe { (*(p as *mut HostCtx)).host.host_data = p as *mut c_void };
        ctx
    }
}

/// A loaded `.clap` file, its entry initialised. Kept for the app's life
/// (unloading code a plugin may still reference is a classic crash).
struct Bundle {
    _lib: Box<dyn NativeLibrary>,
    entry: *const clap_plugin_entry,
    factory: *const clap_plugin_factory,
}

unsafe impl Send for Bundle {}
unsafe impl Sync for Bundle {}

impl Bundle {
    fn load(libs: &dyn NativeLibraries, path: &str) -> Result<Bundle, String> {
        let lib = libs.load(path).map_err(|e| e.to_string())?;
        let entry = lib.symbol("clap_entry").map_err(|e| e.to_string())? as *const clap_plugin_entry;
        unsafe {
            if (*entry).clap_version.major < 1 {
                return Err("not a CLAP 1.x plugin".into());
            }
            let c = CString::new(path).map_err(|e| e.to_string())?;
            if !((*entry).init)(c.as_ptr()) {
                return Err("the plugin declined to load".into());
            }
            let factory = ((*entry).get_factory)(CLAP_PLUGIN_FACTORY_ID.as_ptr() as _) as *const clap_plugin_factory;
            if factory.is_null() {
                ((*entry).deinit)();
                return Err("no plugin factory".into());
            }
            Ok(Bundle { _lib: lib, entry, factory })
        }
    }
}

impl Drop for Bundle {
    fn drop(&mut self) {
        unsafe { ((*self.entry).deinit)() };
    }
}

fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

fn name_of(chars: &[c_char]) -> String {
    let bytes: Vec<u8> = chars.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// How a CLAP parameter maps to the model: a value, or a choice whose
/// index `i` is the plugin value `min + i`.
#[derive(Clone, Debug)]
struct ParamMap {
    id: clap_id,
    min: f64,
    choice: bool,
}

/// What scanning learns of one effect: id, name, vendor, parameters (as
/// the model sees them, and how they map back).
type Scanned = (String, String, String, Vec<ParamInfo>, Vec<ParamMap>);

/// One plugin the host offers.
struct Known {
    bundle: Arc<Bundle>,
    id: CString,
    name: String,
    params: Vec<ParamMap>,
}

/// The CLAP host: what was found, and the main thread.
pub struct ClapHost {
    main: Arc<MainThread>,
    known: HashMap<String, Known>,
    infos: Vec<EffectInfo>,
    /// Files that could not be used, and why.
    pub problems: Vec<(String, String)>,
}

/// A created, initialised plugin (on the main thread), destroyed on drop.
struct Created {
    plugin: *const clap_plugin,
    ctx: Arc<HostCtx>,
}

unsafe impl Send for Created {}

impl Created {
    /// Create and `init` plugin `id` from `bundle`. Main thread.
    fn new(main: &MainRef, bundle: &Bundle, id: &CStr, name: &str) -> Result<Created, String> {
        let ctx = HostCtx::new(main, name);
        let plugin = unsafe { ((*bundle.factory).create_plugin)(bundle.factory, &ctx.host, id.as_ptr()) };
        if plugin.is_null() {
            return Err("the plugin could not be created".into());
        }
        ctx.plugin.store(plugin as *mut _, Ordering::SeqCst);
        if !unsafe { ((*plugin).init)(plugin) } {
            unsafe { ((*plugin).destroy)(plugin) };
            return Err("the plugin failed to initialise".into());
        }
        Ok(Created { plugin, ctx })
    }

    fn ext<T>(&self, id: &[u8]) -> Option<&T> {
        let p = unsafe { ((*self.plugin).get_extension)(self.plugin, id.as_ptr() as _) } as *const T;
        (!p.is_null()).then(|| unsafe { &*p })
    }

    /// Channels of the main input and output port (0 when absent).
    fn main_channels(&self) -> (u32, u32) {
        let Some(ports) = self.ext::<clap_plugin_audio_ports>(CLAP_EXT_AUDIO_PORTS) else { return (2, 2) };
        let main = |input: bool| unsafe {
            let n = (ports.count)(self.plugin, input);
            (0..n)
                .filter_map(|i| {
                    let mut info: clap_audio_port_info = std::mem::zeroed();
                    (ports.get)(self.plugin, i, input, &mut info).then_some(info)
                })
                .find(|p| p.flags & CLAP_AUDIO_PORT_IS_MAIN != 0)
                .map_or(0, |p| p.channel_count)
        };
        (main(true), main(false))
    }

    /// The visible parameters, as the model sees them and as they map back.
    fn params(&self) -> (Vec<ParamInfo>, Vec<ParamMap>) {
        let Some(params) = self.ext::<clap_plugin_params>(CLAP_EXT_PARAMS) else { return (vec![], vec![]) };
        let mut infos = Vec::new();
        let mut maps = Vec::new();
        let n = unsafe { (params.count)(self.plugin) };
        for i in 0..n {
            let mut p: clap_param_info = unsafe { std::mem::zeroed() };
            if !unsafe { (params.get_info)(self.plugin, i, &mut p) } || p.flags & (CLAP_PARAM_IS_HIDDEN | CLAP_PARAM_IS_READONLY) != 0 {
                continue;
            }
            let stepped = p.flags & (CLAP_PARAM_IS_STEPPED | CLAP_PARAM_IS_ENUM) != 0;
            let steps = (p.max_value - p.min_value).round() as i64;
            // A few named values: a choice, labelled by the plugin.
            let labels: Option<Vec<String>> = (stepped && (1..=64).contains(&steps)).then(|| {
                (0..=steps)
                    .map(|k| {
                        let mut buf = [0 as c_char; 128];
                        let v = p.min_value + k as f64;
                        if unsafe { (params.value_to_text)(self.plugin, p.id, v, buf.as_mut_ptr(), buf.len() as u32) } {
                            name_of(&buf)
                        } else {
                            format!("{v}")
                        }
                    })
                    .collect()
            });
            let choice = labels.is_some();
            let (kind, default) = match labels {
                Some(l) => (ParamKind::Choice(l), p.default_value - p.min_value),
                None if stepped => (ParamKind::Int, p.default_value),
                None => (ParamKind::Float, p.default_value),
            };
            infos.push(ParamInfo {
                id: p.id.to_string(),
                label: name_of(&p.name),
                kind,
                min: if choice { 0.0 } else { p.min_value },
                max: if choice { steps as f64 } else { p.max_value },
                default: [default, 0.0, 0.0, 0.0],
                animatable: p.flags & CLAP_PARAM_IS_AUTOMATABLE != 0 && !choice,
            });
            maps.push(ParamMap { id: p.id, min: p.min_value, choice });
        }
        (infos, maps)
    }

    fn destroy(self) {
        self.ctx.plugin.store(std::ptr::null_mut(), Ordering::SeqCst);
        unsafe { ((*self.plugin).destroy)(self.plugin) };
    }
}

impl ClapHost {
    /// Load every CLAP file `libs` finds and list its audio effects.
    pub fn scan(libs: &dyn NativeLibraries) -> ClapHost {
        let paths = discover(libs);
        Self::scan_paths(libs, &paths)
    }

    /// As [`ClapHost::scan`], for given files.
    pub fn scan_paths(libs: &dyn NativeLibraries, paths: &[String]) -> ClapHost {
        let main = MainThread::start();
        let mut host = ClapHost { main: main.clone(), known: HashMap::new(), infos: Vec::new(), problems: Vec::new() };
        for path in paths {
            let bundle = match Bundle::load(libs, path) {
                Ok(b) => Arc::new(b),
                Err(e) => {
                    host.problems.push((path.clone(), e));
                    continue;
                }
            };
            let main_id = main.handle();
            let b = bundle.clone();
            // Descriptors, and each effect's ports and parameters, on the main thread.
            let found: Vec<Result<Scanned, String>> = main.run(move || {
                let f = b.factory;
                let n = unsafe { ((*f).get_plugin_count)(f) };
                (0..n)
                    .filter_map(|i| {
                        let d = unsafe { ((*f).get_plugin_descriptor)(f, i) };
                        if d.is_null() {
                            return None;
                        }
                        let d = unsafe { &*d };
                        let mut features = Vec::new();
                        let mut fp = d.features;
                        while !fp.is_null() && !unsafe { *fp }.is_null() {
                            features.push(cstr(unsafe { *fp }));
                            fp = unsafe { fp.add(1) };
                        }
                        if !features.iter().any(|f| f == FEATURE_AUDIO_EFFECT) {
                            // Instruments, analyzers, note effects: said, not silently dropped.
                            return Some(Err(format!("{}: not an audio effect ({})", cstr(d.name), features.join(", "))));
                        }
                        let (id, name, vendor) = (cstr(d.id), cstr(d.name), cstr(d.vendor));
                        let cid = CString::new(id.clone()).ok()?;
                        Some(Created::new(&main_id, &b, &cid, &name).and_then(|c| {
                            let (inputs, _) = c.main_channels();
                            let (infos, maps) = c.params();
                            c.destroy();
                            if inputs == 0 {
                                return Err(format!("{name}: no audio input"));
                            }
                            Ok((id, name, vendor, infos, maps))
                        }))
                    })
                    .collect()
            });
            for r in found {
                match r {
                    Ok((id, name, vendor, params, maps)) => {
                        host.infos.push(EffectInfo {
                            plugin: PluginRef { api: PluginApi::Clap, id: id.clone(), major_version: 1 },
                            name: name.clone(),
                            category: if vendor.is_empty() { "CLAP".into() } else { format!("CLAP · {vendor}") },
                            kind: EffectKind::AudioFilter,
                            params,
                            wgsl: None,
                            implementation: Implementation::Mixer,
                            reads_levels: false,
                        });
                        let cid = CString::new(id.clone()).unwrap_or_default();
                        host.known.insert(id, Known { bundle: bundle.clone(), id: cid, name, params: maps });
                    }
                    Err(e) => host.problems.push((path.clone(), e)),
                }
            }
        }
        host
    }

    /// The effects found, for the registry.
    pub fn effects(&self) -> &[EffectInfo] {
        &self.infos
    }
}

impl AudioPlugins for ClapHost {
    fn instantiate(&self, plugin: &PluginRef, sample_rate: u32) -> Result<Box<dyn AudioProcessor>, String> {
        let k = self.known.get(&plugin.id).filter(|_| plugin.api == PluginApi::Clap).ok_or_else(|| format!("{}: not installed", plugin.id))?;
        let (bundle, id, name, main_id) = (k.bundle.clone(), k.id.clone(), k.name.clone(), self.main.handle());
        let created = self.main.run(move || -> Result<(Created, u32, u32), String> {
            let c = Created::new(&main_id, &bundle, &id, &name)?;
            let (inputs, outputs) = c.main_channels();
            if !unsafe { ((*c.plugin).activate)(c.plugin, sample_rate as f64, 1, MAX_FRAMES) } {
                c.destroy();
                return Err(format!("{name} could not start at {sample_rate} Hz"));
            }
            Ok((c, inputs.clamp(1, 2), outputs.clamp(1, 2)))
        })?;
        let (c, inputs, outputs) = created;
        self.main.callbacks.lock().unwrap().push(c.ctx.clone());
        let n = MAX_FRAMES as usize;
        Ok(Box::new(Instance {
            created: Some(c),
            main: self.main.clone(),
            params: k.params.clone(),
            sent: HashMap::new(),
            processing: false,
            inputs,
            outputs,
            buf_in: [vec![0.0; n], vec![0.0; n]],
            buf_out: [vec![0.0; n], vec![0.0; n]],
            events: Vec::with_capacity(k.params.len()),
            steady: 0,
        }))
    }
}

/// One plugin, processing a clip's sound.
struct Instance {
    created: Option<Created>,
    main: Arc<MainThread>,
    params: Vec<ParamMap>,
    /// The plugin value each parameter was last given.
    sent: HashMap<clap_id, f64>,
    processing: bool,
    inputs: u32,
    outputs: u32,
    buf_in: [Vec<f32>; 2],
    buf_out: [Vec<f32>; 2],
    events: Vec<clap_event_param_value>,
    steady: i64,
}

// The plugin pointer is used on the mixer's thread only (and destroyed via
// the main thread); the events hold null cookies.
unsafe impl Send for Instance {}

unsafe extern "C" fn events_size(list: *const clap_input_events) -> u32 {
    unsafe { (*((*list).ctx as *const Vec<clap_event_param_value>)).len() as u32 }
}
unsafe extern "C" fn events_get(list: *const clap_input_events, i: u32) -> *const clap_event_header {
    let v = unsafe { &*((*list).ctx as *const Vec<clap_event_param_value>) };
    v.get(i as usize).map_or(std::ptr::null(), |e| &e.header as *const _)
}
unsafe extern "C" fn events_push(_: *const clap_output_events, _: *const clap_event_header) -> bool {
    true // gestures and values the plugin reports are not recorded yet
}

impl AudioProcessor for Instance {
    fn process(&mut self, params: &[(&str, f64)], left: &mut [f32], right: &mut [f32]) {
        let Some(c) = &self.created else { return };
        let plugin = c.plugin;
        if !self.processing {
            self.processing = unsafe { ((*plugin).start_processing)(plugin) };
            if !self.processing {
                return;
            }
        }
        // Changed parameters, as events at the block's start.
        self.events.clear();
        for (name, v) in params {
            let Ok(id) = name.parse::<clap_id>() else { continue };
            let Some(map) = self.params.iter().find(|m| m.id == id) else { continue };
            let value = if map.choice { map.min + v } else { *v };
            if self.sent.get(&id) != Some(&value) {
                self.sent.insert(id, value);
                self.events.push(clap_event_param_value {
                    header: clap_event_header {
                        size: std::mem::size_of::<clap_event_param_value>() as u32,
                        time: 0,
                        space_id: CLAP_CORE_EVENT_SPACE_ID,
                        type_: CLAP_EVENT_PARAM_VALUE,
                        flags: 0,
                    },
                    param_id: id,
                    cookie: std::ptr::null_mut(),
                    note_id: -1,
                    port_index: -1,
                    channel: -1,
                    key: -1,
                    value,
                });
            }
        }
        let mut done = 0;
        while done < left.len() {
            let n = (left.len() - done).min(MAX_FRAMES as usize);
            let (l, r) = (&mut left[done..done + n], &mut right[done..done + n]);
            if self.inputs == 1 {
                for k in 0..n {
                    self.buf_in[0][k] = (l[k] + r[k]) * 0.5;
                }
            } else {
                self.buf_in[0][..n].copy_from_slice(l);
                self.buf_in[1][..n].copy_from_slice(r);
            }
            let mut ins: [*mut f32; 2] = [self.buf_in[0].as_mut_ptr(), self.buf_in[1].as_mut_ptr()];
            let mut outs: [*mut f32; 2] = [self.buf_out[0].as_mut_ptr(), self.buf_out[1].as_mut_ptr()];
            let input = clap_audio_buffer { data32: ins.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: self.inputs, latency: 0, constant_mask: 0 };
            let mut output = clap_audio_buffer { data32: outs.as_mut_ptr(), data64: std::ptr::null_mut(), channel_count: self.outputs, latency: 0, constant_mask: 0 };
            // The events go with the first slice only.
            let empty: Vec<clap_event_param_value> = Vec::new();
            let list = if done == 0 { &self.events } else { &empty };
            let in_events = clap_input_events { ctx: list as *const _ as *mut c_void, size: events_size, get: events_get };
            let out_events = clap_output_events { ctx: std::ptr::null_mut(), try_push: events_push };
            let proc_ = clap_process {
                steady_time: self.steady,
                frames_count: n as u32,
                transport: std::ptr::null(),
                audio_inputs: &input,
                audio_outputs: &mut output,
                audio_inputs_count: 1,
                audio_outputs_count: 1,
                in_events: &in_events,
                out_events: &out_events,
            };
            let status = unsafe { ((*plugin).process)(plugin, &proc_) };
            self.steady += n as i64;
            if status != CLAP_PROCESS_ERROR {
                l.copy_from_slice(&self.buf_out[0][..n]);
                let right_out = if self.outputs == 1 { 0 } else { 1 };
                r.copy_from_slice(&self.buf_out[right_out][..n]);
            }
            done += n;
        }
    }

    fn reset(&mut self) {
        if let Some(c) = &self.created {
            unsafe { ((*c.plugin).reset)(c.plugin) };
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        let Some(c) = self.created.take() else { return };
        // Processing stops on the audio thread (this one); the rest on main.
        if self.processing {
            unsafe { ((*c.plugin).stop_processing)(c.plugin) };
        }
        let callbacks = self.main.callbacks.clone();
        self.main.post(move || {
            callbacks.lock().unwrap().retain(|x| !Arc::ptr_eq(x, &c.ctx));
            unsafe { ((*c.plugin).deactivate)(c.plugin) };
            c.destroy();
        });
    }
}
