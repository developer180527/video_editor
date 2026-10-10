//! The engine on its own thread.
//!
//! [`Engine::spawn`] moves the engine onto a thread and returns an
//! [`EngineClient`]. The client sends requests (never blocking on the work)
//! and reads the latest [`Published`] state, which the engine thread replaces
//! after every request. Imports, saves and probing run there, so the UI never
//! waits on a disk or a decoder.
//!
//! The playhead is computed on the caller's side from the published
//! transport and the audio device's shared frame counter, so it moves smoothly
//! without a round trip per frame.

use std::sync::{mpsc, Arc, Mutex};

use ve_model::{MediaRef, Snapshot};
use ve_playback::{Clocks, State, Transport};
use ve_ports::Capabilities;
use ve_render::{FramePlan, Quality};
use ve_time::Time;

use crate::{Command, Engine, Event, PluginRegistry};

/// How often unsaved changes are written to the autosave file.
const AUTOSAVE_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// Called by the engine thread after it publishes, so a sleeping UI wakes.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Everything a frontend reads, as of the engine's last step.
#[derive(Clone)]
pub struct Published {
    pub snapshot: Snapshot,
    pub undo_label: Option<String>,
    pub redo_label: Option<String>,
    pub dirty: bool,
    pub file: Option<MediaRef>,
    pub transport: Transport,
    pub audio_clock: Option<Arc<ve_ports::AudioClock>>,
    pub plugins: Arc<PluginRegistry>,
    pub capabilities: Capabilities,
    /// Work in progress, for a status line ("Importing clip.mov").
    pub busy: Option<String>,
    /// Audio levels, once the audio device is open.
    pub meters: Option<Arc<ve_media::Meters>>,
    /// Which monitor `transport` plays; the other waits in `parked`.
    pub viewer: crate::Viewer,
    pub parked: Transport,
    pub source: Option<crate::SourceView>,
    /// Counts the client's own transport changes (play, seek, switching
    /// monitors). Until the engine has adopted the latest, a publish keeps
    /// the client's transport: otherwise an edit's publish, made before the
    /// engine saw a seek, would put the playhead back for a moment.
    pub transport_rev: u64,
}

enum Request {
    Execute(Command),
    ExecuteMerging(Command, u64),
    Undo,
    Redo,
    Import(Vec<String>),
    NewProject(String),
    SaveAs(MediaRef),
    Open(MediaRef),
    /// The transport exactly as the client set it (play, stop, seek), so
    /// both sides share one anchor and the playhead never jumps.
    Transport(Transport, u64),
    Looping(bool),
    AttachProxy(ve_model::AssetId, String),
    Relink(ve_model::AssetId, String),
    PreviewTrack(ve_model::TrackId, Option<(f32, f32)>),
    SetSource(ve_model::AssetId),
    /// The monitors exactly as the client switched them.
    Viewer(crate::Viewer, Transport, Transport, u64),
}

pub struct EngineClient {
    platform: ve_ports::Platform,
    video: Arc<ve_media::VideoPool>,
    stills: Arc<ve_media::Stills>,
    tx: mpsc::Sender<Request>,
    shared: Arc<Mutex<Published>>,
    events: mpsc::Receiver<Event>,
}

fn publish(e: &Engine, busy: Option<String>) -> Published {
    Published {
        snapshot: e.snapshot(),
        undo_label: e.undo_label().map(String::from),
        redo_label: e.redo_label().map(String::from),
        dirty: e.is_dirty(),
        file: e.file().cloned(),
        transport: e.transport().clone(),
        audio_clock: e.audio_clock(),
        plugins: e.plugins().clone(),
        capabilities: e.capabilities(),
        busy,
        meters: e.meters(),
        viewer: e.viewer(),
        parked: e.parked().clone(),
        source: e.source().cloned(),
        transport_rev: 0,
    }
}

impl Engine {
    /// Run the engine on its own thread. `waker` is called whenever there is
    /// something new to show.
    pub fn spawn(mut self, waker: Waker) -> EngineClient {
        let (tx, rx) = mpsc::channel::<Request>();
        let (etx, events) = mpsc::channel::<Event>();
        let shared = Arc::new(Mutex::new(publish(&self, None)));
        let video = self.video().clone();
        let platform = self.platform().clone();
        video.set_waker(waker.clone());
        let stills = self.stills().clone();
        stills.set_waker(waker.clone());
        let out = shared.clone();
        std::thread::Builder::new()
            .name("ve-engine".into())
            .spawn(move || {
                // The client's transport changes the engine has adopted.
                let mut adopted = 0u64;
                let flush = |e: &mut Engine, busy: Option<String>, adopted: u64| {
                    let mut next = publish(e, busy);
                    let mut shared = out.lock().unwrap();
                    next.transport_rev = shared.transport_rev;
                    if shared.transport_rev > adopted {
                        // The client has moved on since: keep its transport.
                        next.transport = shared.transport.clone();
                        next.parked = shared.parked.clone();
                        next.viewer = shared.viewer;
                    }
                    *shared = next;
                    drop(shared);
                    for ev in e.drain_events() {
                        let _ = etx.send(ev);
                    }
                    waker();
                };
                // Open the audio device now, not on the first Play: opening
                // takes a moment, and the clock should not change basis mid-play.
                self.prepare_audio();
                flush(&mut self, None, adopted);
                // Ends when the client is dropped. Between requests (and at
                // least every AUTOSAVE_EVERY) unsaved changes are autosaved.
                let mut last_autosave = std::time::Instant::now();
                loop {
                    let req = match rx.recv_timeout(AUTOSAVE_EVERY) {
                        Ok(r) => Some(r),
                        Err(mpsc::RecvTimeoutError::Timeout) => None,
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    };
                    if last_autosave.elapsed() >= AUTOSAVE_EVERY {
                        last_autosave = std::time::Instant::now();
                        if let Err(err) = self.autosave() {
                            let _ = etx.send(Event::Error(format!("Autosave failed: {err}")));
                        }
                    }
                    let Some(req) = req else { continue };
                    match req {
                        Request::Execute(c) => {
                            if let Err(err) = self.execute(c) {
                                let _ = etx.send(Event::Error(err.to_string()));
                            }
                        }
                        Request::ExecuteMerging(c, key) => {
                            if let Err(err) = self.execute_merging(c, key) {
                                let _ = etx.send(Event::Error(err.to_string()));
                            }
                        }
                        Request::Undo => {
                            self.undo();
                        }
                        Request::Redo => {
                            self.redo();
                        }
                        Request::Import(paths) => {
                            for p in paths {
                                let name = std::path::Path::new(&p).file_name().map(|n| n.to_string_lossy().into_owned());
                                flush(&mut self, Some(format!("Importing {}", name.unwrap_or_else(|| p.clone()))), adopted);
                                if let Err(err) = self.import(&p) {
                                    let _ = etx.send(Event::Error(format!("{p}: {err}")));
                                }
                            }
                        }
                        Request::NewProject(n) => self.new_project(&n),
                        Request::SaveAs(r) => {
                            if let Err(err) = self.save_as(r) {
                                let _ = etx.send(Event::Error(format!("Save failed: {err}")));
                            }
                        }
                        Request::Open(r) => {
                            if let Err(err) = self.open(r) {
                                let _ = etx.send(Event::Error(format!("Open failed: {err}")));
                            }
                        }
                        Request::Transport(t, rev) => {
                            adopted = adopted.max(rev);
                            self.adopt_transport(t)
                        }
                        Request::Looping(on) => self.set_looping(on),
                        Request::PreviewTrack(track, mix) => self.preview_track(track, mix),
                        Request::SetSource(asset) => {
                            if let Err(err) = self.set_source(asset) {
                                let _ = etx.send(Event::Error(err.to_string()));
                            }
                        }
                        Request::Viewer(v, active, parked, rev) => {
                            adopted = adopted.max(rev);
                            self.adopt_viewer(v, active, parked)
                        }
                        Request::AttachProxy(asset, path) => {
                            if let Err(err) = self.attach_proxy(asset, &path) {
                                let _ = etx.send(Event::Error(format!("{path}: {err}")));
                            }
                        }
                        Request::Relink(asset, path) => {
                            if let Err(err) = self.relink(asset, &path) {
                                let _ = etx.send(Event::Error(format!("{path}: {err}")));
                            }
                        }
                    }
                    flush(&mut self, None, adopted);
                }
            })
            .expect("engine thread");
        EngineClient { platform, video, stills, tx, shared, events }
    }
}

impl EngineClient {
    /// The latest published state (cheap: a handful of `Arc`s).
    pub fn published(&self) -> Published {
        self.shared.lock().unwrap().clone()
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().unwrap().snapshot.clone()
    }

    pub fn plugins(&self) -> Arc<PluginRegistry> {
        self.shared.lock().unwrap().plugins.clone()
    }

    pub fn capabilities(&self) -> Capabilities {
        self.shared.lock().unwrap().capabilities.clone()
    }

    fn send(&self, r: Request) {
        let _ = self.tx.send(r);
    }

    pub fn execute(&self, c: Command) {
        self.send(Request::Execute(c));
    }

    /// `c` as part of the gesture `key` (a slider drag): one undo step for
    /// the whole gesture. Use a new key for each gesture.
    pub fn execute_merging(&self, c: Command, key: u64) {
        self.send(Request::ExecuteMerging(c, key));
    }

    pub fn undo(&self) {
        self.send(Request::Undo);
    }

    pub fn redo(&self) {
        self.send(Request::Redo);
    }

    pub fn import(&self, paths: Vec<String>) {
        self.send(Request::Import(paths));
    }

    pub fn new_project(&self, name: &str) {
        self.send(Request::NewProject(name.into()));
    }

    /// Attach the file at `path` as `asset`'s proxy (probed on the engine
    /// thread).
    pub fn attach_proxy(&self, asset: ve_model::AssetId, path: String) {
        self.send(Request::AttachProxy(asset, path));
    }

    /// Hear `track` at this fader (dB) and pan while it is dragged, without
    /// an edit (no restart, no undo step); `None` ends the preview. Commit
    /// the final values with `SetTrackState`.
    pub fn preview_track(&self, track: ve_model::TrackId, mix: Option<(f32, f32)>) {
        self.send(Request::PreviewTrack(track, mix));
    }

    /// Relink `asset` to the file at `path`.
    pub fn relink(&self, asset: ve_model::AssetId, path: String) {
        self.send(Request::Relink(asset, path));
    }

    pub fn save_as(&self, to: MediaRef) {
        self.send(Request::SaveAs(to));
    }

    pub fn open(&self, from: MediaRef) {
        self.send(Request::Open(from));
    }

    fn clocks(&self, p: &Published) -> Clocks {
        Clocks { audio: p.audio_clock.as_ref().map(|c| c.read()), monotonic: ve_ports::clock_now() }
    }

    /// Transport changes apply to the local copy at once, so the playhead
    /// responds this frame; the engine then adopts exactly that state.
    fn transport(&self, f: impl FnOnce(&mut Transport, Clocks)) {
        let t = {
            let mut p = self.shared.lock().unwrap();
            let now = self.clocks(&p);
            f(&mut p.transport, now);
            p.transport_rev += 1;
            (p.transport.clone(), p.transport_rev)
        };
        self.send(Request::Transport(t.0, t.1));
    }

    pub fn play(&self, rate: f64) {
        self.transport(|t, now| t.play(rate, now));
    }

    pub fn stop(&self) {
        self.transport(|t, now| t.stop(now));
    }

    pub fn seek(&self, t: Time) {
        self.transport(|tr, now| tr.seek(t, now));
    }

    pub fn set_looping(&self, on: bool) {
        self.transport(|t, _| t.looping = on);
        self.send(Request::Looping(on));
    }

    /// Which monitor the transport plays.
    pub fn viewer(&self) -> crate::Viewer {
        self.shared.lock().unwrap().viewer
    }

    pub fn source(&self) -> Option<crate::SourceView> {
        self.shared.lock().unwrap().source.clone()
    }

    /// Open `asset` in the Source monitor (it becomes the active monitor).
    pub fn set_source(&self, asset: ve_model::AssetId) {
        self.send(Request::SetSource(asset));
    }

    /// Make `v` the monitor the transport plays: the other stops where it
    /// is and waits. Switched here at once, so playheads never jump.
    pub fn set_viewer(&self, v: crate::Viewer) {
        let msg = {
            let mut p = self.shared.lock().unwrap();
            if p.viewer == v || (v == crate::Viewer::Source && p.source.is_none()) {
                return;
            }
            let now = self.clocks(&p);
            p.transport.stop(now);
            let p = &mut *p;
            std::mem::swap(&mut p.transport, &mut p.parked);
            p.viewer = v;
            p.transport_rev += 1;
            Request::Viewer(v, p.transport.clone(), p.parked.clone(), p.transport_rev)
        };
        self.send(msg);
    }

    /// `v`'s playhead, whether or not it is the active monitor.
    pub fn playhead_of(&self, v: crate::Viewer) -> Time {
        let p = self.shared.lock().unwrap();
        let now = self.clocks(&p);
        if p.viewer == v { p.transport.position_at(now) } else { p.parked.position_at(now) }
    }

    pub fn is_playing(&self) -> bool {
        matches!(self.shared.lock().unwrap().transport.state(), State::Playing { .. })
    }

    pub fn playhead(&self) -> Time {
        let p = self.shared.lock().unwrap();
        p.transport.position_at(self.clocks(&p))
    }

    /// What the active sequence shows at the playhead.
    pub fn frame_plan(&self, quality: Quality) -> Option<FramePlan> {
        let snap = self.snapshot();
        let t = self.playhead();
        snap.active().map(|s| ve_render::evaluate(s, t, quality))
    }

    /// The Program monitor's frame at its playhead, resolved for the
    /// compositor with whatever the decoders have ready. Never blocks.
    pub fn frame(&self, quality: Quality) -> Option<crate::Frame> {
        self.frame_of(crate::Viewer::Program, quality)
    }

    /// Monitor `v`'s frame at its playhead. Never blocks.
    pub fn frame_of(&self, v: crate::Viewer, quality: Quality) -> Option<crate::Frame> {
        let p = self.published();
        let now = self.clocks(&p);
        let transport = if p.viewer == v { &p.transport } else { &p.parked };
        let t = transport.position_at(now);
        // The Source monitor's asset plays as a sequence of its own.
        let project = match v {
            crate::Viewer::Program => p.snapshot.clone(),
            crate::Viewer::Source => p.source.as_ref()?.project(&p.snapshot),
        };
        let plan = project.active().map(|s| ve_render::evaluate(s, t, quality))?;
        // Playing forwards: have the next cut's pictures ready in time.
        if let State::Playing { rate } = transport.state() {
            if rate > 0.0 {
                crate::frame::prefetch(&project, t, Time::from_seconds_f64(2.0 * rate), quality.use_proxies, &self.video);
            }
        }
        Some(crate::frame::resolve(&project, plan, &p.plugins, &self.video, None))
    }

    pub fn video(&self) -> &Arc<ve_media::VideoPool> {
        &self.video
    }

    pub fn stills(&self) -> &Arc<ve_media::Stills> {
        &self.stills
    }

    /// Export the active sequence as it is now to `out`, on its own thread,
    /// rendering on `gpu`, importing hardware-decoded frames with `importer`
    /// when there is one.
    pub fn export(
        &self,
        gpu: (wgpu::Device, wgpu::Queue),
        preset: crate::ExportPreset,
        out: MediaRef,
        importer: Option<Arc<dyn ve_render::TextureImporter>>,
    ) -> Arc<crate::ExportState> {
        let p = self.published();
        crate::export::start(p.snapshot, self.platform.clone(), p.plugins, gpu, preset, out, importer)
    }

    pub fn drain_events(&self) -> Vec<Event> {
        self.events.try_iter().collect()
    }
}
