//! The engine API: the only way in or out of the editor's core.
//!
//! A frontend — the desktop UI, the iPad UI, the CLI, a test, a script, an
//! add-on — does three things:
//!
//! 1. **Sends commands** ([`Engine::execute`]); the engine validates, applies,
//!    records undo, and publishes a new snapshot.
//! 2. **Reads snapshots** ([`Engine::snapshot`]): an immutable, shared view of
//!    the document, safe to hold while drawing.
//! 3. **Drains events** ([`Engine::drain_events`]) to learn what changed.
//!
//! Frontends never hold `&mut Project`, so no frontend can corrupt a project.
//! Today the engine runs on the caller's thread; moving it behind a channel
//! (or a socket) changes nothing for callers.

mod client;
mod clips;
pub mod export;
pub mod frame;
mod project_file;

pub use client::{EngineClient, Published, Waker};
pub use clips::{default_value, make_clip};
pub use ve_command::edit;
pub use ve_plugin_host::intrinsic;

pub use project_file::{ProjectFileError, FILE_FORMAT};
pub use ve_command::{Command, CommandError, Edge, TrackState};
pub use ve_model::Snapshot;
pub use ve_plugin_host::Registry as PluginRegistry;
/// How plugins describe themselves, for frontends that build UI from it.
pub use ve_plugin_host::{EffectInfo, EffectKind, Implementation, ParamInfo, ParamKind};
pub use ve_render::{FramePlan, Quality, TextureImporter};
pub use frame::Frame;
pub use export::{ExportPreset, ExportState};
pub use ve_media::{Meters, Stills, Thumb, PEAKS_PER_SECOND};

use std::sync::Arc;
use thiserror::Error;
use ve_command::History;
use ve_model::*;
pub use ve_playback::{Clocks, State, Transport};
use ve_plugin_host::native;
use ve_ports::{AudioConfig, AudioStream, Capabilities, MediaError, Platform, StorageError};
use ve_time::Time;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error(transparent)]
    Command(#[from] CommandError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Media(#[from] MediaError),
    #[error(transparent)]
    File(#[from] ProjectFileError),
}

/// What changed, for frontends to react to.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A new snapshot is available.
    ProjectChanged,
    /// Play, stop, or a seek.
    TransportChanged,
    PluginsChanged,
    /// Something failed that the user should hear about.
    Error(String),
}

pub struct Engine {
    platform: Platform,
    project: Snapshot,
    history: History,
    plugins: Arc<PluginRegistry>,
    transport: Transport,
    audio: Option<Box<dyn AudioStream>>,
    playback: Option<ve_media::Playback>,
    video: Arc<ve_media::VideoPool>,
    stills: Arc<ve_media::Stills>,
    file: Option<MediaRef>,
    /// Identifies this editing session's autosave file.
    session: SequenceId,
    /// The snapshot last autosaved, to skip unchanged ones.
    autosaved: Option<Snapshot>,
    events: Vec<Event>,
}

/// The sample rate the engine mixes at.
pub const MIX_RATE: u32 = 48_000;

impl Engine {
    pub fn new(platform: Platform) -> Self {
        let budget = (platform.capabilities().memory_budget / 8) as usize;
        let video = ve_media::VideoPool::new(platform.storage.clone(), platform.media.clone(), budget);
        let stills = ve_media::Stills::new(platform.storage.clone(), platform.media.clone());
        let mut e = Engine {
            platform,
            project: Arc::new(Project::new("Untitled")),
            history: History::default(),
            plugins: Arc::new(PluginRegistry::default()),
            transport: Transport::new(MIX_RATE),
            audio: None,
            playback: None,
            video,
            stills,
            file: None,
            session: SequenceId::new(),
            autosaved: None,
            events: Vec::new(),
        };
        e.load_plugins();
        e
    }

    pub fn capabilities(&self) -> Capabilities {
        self.platform.capabilities()
    }

    pub fn plugins(&self) -> &Arc<PluginRegistry> {
        &self.plugins
    }

    /// Native plugins: everything linked into the app, plus — where the
    /// platform allows loading code — every `ve` plugin found on disk.
    fn load_plugins(&mut self) {
        let mut registry = PluginRegistry::default();
        ve_plugin_host::intrinsic::all().into_iter().for_each(|e| registry.add(e));
        let libs = self.platform.libraries.clone();
        let mut found: Vec<(Arc<dyn ve_ports::NativeLibrary>, Option<String>)> =
            libs.linked().into_iter().map(|l| (Arc::from(l), None)).collect();
        for (lib, name) in &mut found {
            *name = Some(lib.name().to_string());
        }
        if self.capabilities().dynamic_libraries {
            for path in libs.discover("ve") {
                match libs.load(&path) {
                    Ok(l) => found.push((Arc::from(l), None)),
                    Err(e) => self.events.push(Event::Error(format!("{path}: {e}"))),
                }
            }
        }
        for (lib, static_name) in found {
            match native::load(lib, static_name.as_deref()) {
                Ok(effects) => effects.into_iter().for_each(|e| registry.add(e)),
                Err(e) => self.events.push(Event::Error(e.to_string())),
            }
        }
        self.plugins = Arc::new(registry);
        self.events.push(Event::PluginsChanged);
    }

    // ---- the document ----------------------------------------------------

    pub fn snapshot(&self) -> Snapshot {
        self.project.clone()
    }

    /// A fresh project with one 1080p24 sequence: V1, V2, A1, A2.
    pub fn new_project(&mut self, name: &str) {
        let mut p = Project::new(name);
        let seq = Sequence {
            id: SequenceId::new(),
            name: "Sequence 01".into(),
            format: SequenceFormat::default(),
            tracks: [
                Track::new(TrackKind::Video, "V1"),
                Track::new(TrackKind::Video, "V2"),
                Track::new(TrackKind::Audio, "A1"),
                Track::new(TrackKind::Audio, "A2"),
            ]
            .into_iter()
            .map(Arc::new)
            .collect(),
        };
        p.active_sequence = Some(seq.id);
        p.sequences.insert(seq.id, Arc::new(seq));
        self.replace_project(p, None);
    }

    fn replace_project(&mut self, p: Project, file: Option<MediaRef>) {
        self.project = Arc::new(p);
        self.history = History::default();
        self.history.mark_saved();
        self.file = file;
        self.session = SequenceId::new();
        self.autosaved = Some(self.project.clone());
        self.transport = Transport::new(MIX_RATE);
        self.events.push(Event::ProjectChanged);
    }

    pub fn execute(&mut self, cmd: Command) -> Result<(), CommandError> {
        let next = self.history.execute(&self.project, cmd)?;
        self.publish(next);
        Ok(())
    }

    pub fn undo(&mut self) -> bool {
        self.step(true)
    }

    pub fn redo(&mut self) -> bool {
        self.step(false)
    }

    fn step(&mut self, undo: bool) -> bool {
        let r = if undo { self.history.undo(&self.project) } else { self.history.redo(&self.project) };
        match r {
            Some(Ok(p)) => {
                self.publish(p);
                true
            }
            Some(Err(e)) => {
                self.events.push(Event::Error(e.to_string()));
                false
            }
            None => false,
        }
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.history.undo_label()
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.history.redo_label()
    }

    pub fn is_dirty(&self) -> bool {
        self.history.is_dirty()
    }

    fn publish(&mut self, p: Project) {
        self.project = Arc::new(p);
        if let Some(seq) = self.project.active() {
            self.transport.end = seq.duration();
        }
        // Edits are heard at once while playing.
        if self.is_playing() {
            self.restart_audio();
        }
        self.events.push(Event::ProjectChanged);
    }

    /// Import a file the user picked: make a lasting reference, probe it, add
    /// it to the bin. Returns the new asset.
    pub fn import(&mut self, picked: &str) -> Result<AssetId, EngineError> {
        let storage = &self.platform.storage;
        let media = storage.make_ref(picked)?;
        let info = self.platform.media.probe(&storage.resolve(&media)?)?;
        let asset = Asset { id: AssetId::new(), name: storage.display_name(&media), media, info: Some(info) };
        let id = asset.id;
        self.execute(Command::AddAsset { asset: Arc::new(asset) })?;
        Ok(id)
    }

    // ---- files -------------------------------------------------------------

    pub fn file(&self) -> Option<&MediaRef> {
        self.file.as_ref()
    }

    /// Save to `to`. The file is replaced atomically: a crash or a full
    /// disk mid-save leaves the previous version intact.
    pub fn save_as(&mut self, to: MediaRef) -> Result<(), EngineError> {
        let mut bytes = Vec::new();
        project_file::write(&self.project, &mut bytes)?;
        self.platform.storage.write_atomic(&to, &bytes)?;
        self.history.mark_saved();
        // Saved for real: this session's autosave is no longer needed.
        if let Ok(a) = self.autosave_ref() {
            let _ = self.platform.storage.remove(&a);
        }
        self.autosaved = Some(self.project.clone());
        self.file = Some(to);
        Ok(())
    }

    /// Where this session autosaves: `Autosave/<name>.<session>.veproj` in
    /// the app's data folder. A file left there is unsaved work from a
    /// session that ended without saving; it opens like any project.
    pub fn autosave_ref(&self) -> Result<MediaRef, StorageError> {
        let storage = &self.platform.storage;
        let name = match &self.file {
            Some(f) => storage.display_name(f).trim_end_matches(".veproj").to_string(),
            None => self.project.name.clone(),
        };
        let name: String = name.chars().map(|c| if c.is_alphanumeric() || " -_".contains(c) { c } else { '_' }).collect();
        storage.location(ve_ports::Location::AppData, &format!("Autosave/{name}.{}.veproj", self.session))
    }

    /// Write the autosave if there are changes since the last save or
    /// autosave. Returns whether it wrote.
    pub fn autosave(&mut self) -> Result<bool, EngineError> {
        if !self.is_dirty() || self.autosaved.as_ref().is_some_and(|a| Arc::ptr_eq(a, &self.project)) {
            return Ok(false);
        }
        let mut bytes = Vec::new();
        project_file::write(&self.project, &mut bytes)?;
        self.platform.storage.write_atomic(&self.autosave_ref()?, &bytes)?;
        self.autosaved = Some(self.project.clone());
        Ok(true)
    }

    pub fn open(&mut self, from: MediaRef) -> Result<(), EngineError> {
        let mut r = self.platform.storage.open_read(&from)?;
        let p = project_file::read(&mut r)?;
        self.replace_project(p, Some(from));
        Ok(())
    }

    // ---- playback --------------------------------------------------------

    fn clocks(&self) -> Clocks {
        Clocks { audio: self.audio_clock().map(|c| c.read()), monotonic: ve_ports::clock_now() }
    }

    /// Open the audio device now (the threaded client does this at start).
    pub fn prepare_audio(&mut self) {
        self.ensure_audio();
    }

    /// Take a transport state set by a client (see `EngineClient`).
    pub fn adopt_transport(&mut self, mut t: Transport) {
        self.ensure_audio();
        t.end = self.transport.end;
        self.transport = t;
        self.restart_audio();
        self.events.push(Event::TransportChanged);
    }

    /// Open the audio device at its own format, with the mixer feeding it.
    fn ensure_audio(&mut self) {
        if self.audio.is_some() {
            return;
        }
        let cfg = self.platform.audio.preferred().unwrap_or(AudioConfig { sample_rate: MIX_RATE, channels: 2, buffer_frames: 512 });
        let mixer = ve_media::Mixer::new(self.platform.storage.clone(), self.platform.media.clone(), cfg.sample_rate);
        let (playback, callback) = ve_media::Playback::new(mixer, cfg.channels);
        match self.platform.audio.open(cfg, callback) {
            Ok(mut s) => {
                let _ = s.play();
                self.transport.set_sample_rate(s.config().sample_rate);
                self.audio = Some(s);
                self.playback = Some(playback);
            }
            Err(e) => self.events.push(Event::Error(format!("Audio output: {e}"))),
        }
    }

    /// (Re)start the mixer at the playhead, when playing forwards at speed.
    fn restart_audio(&mut self) {
        let Some(pb) = &self.playback else { return };
        match (self.transport.state(), self.project.active_sequence) {
            (State::Playing { rate }, Some(seq)) if (rate - 1.0).abs() < 1e-9 => {
                pb.play(self.project.clone(), seq, self.playhead());
            }
            _ => pb.stop(),
        }
    }

    pub fn play(&mut self, rate: f64) {
        self.ensure_audio();
        let now = self.clocks();
        self.transport.play(rate, now);
        self.restart_audio();
        self.events.push(Event::TransportChanged);
    }

    pub fn stop(&mut self) {
        let now = self.clocks();
        self.transport.stop(now);
        self.restart_audio();
        self.events.push(Event::TransportChanged);
    }

    pub fn seek(&mut self, t: Time) {
        let now = self.clocks();
        self.transport.seek(t, now);
        self.restart_audio();
        self.events.push(Event::TransportChanged);
    }

    /// Level meters of what is playing, once audio is open.
    pub fn meters(&self) -> Option<Arc<ve_media::Meters>> {
        self.playback.as_ref().map(|p| p.meters.clone())
    }

    /// Thumbnails and waveform peaks, made in the background.
    pub fn stills(&self) -> &Arc<ve_media::Stills> {
        &self.stills
    }

    /// Background video decode, shared with frontends and export.
    pub fn video(&self) -> &Arc<ve_media::VideoPool> {
        &self.video
    }

    pub fn is_playing(&self) -> bool {
        matches!(self.transport.state(), State::Playing { .. })
    }

    pub fn set_looping(&mut self, on: bool) {
        self.transport.looping = on;
    }

    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    /// The playhead's clock once audio is open: mixed frames the device has
    /// been given (not the silence while the mixer catches up).
    pub fn audio_clock(&self) -> Option<Arc<ve_ports::AudioClock>> {
        self.audio.as_ref().and(self.playback.as_ref()).map(|p| p.clock())
    }

    pub fn platform(&self) -> &Platform {
        &self.platform
    }

    pub fn playhead(&self) -> Time {
        self.transport.position_at(self.clocks())
    }

    /// What the active sequence shows at the playhead.
    pub fn frame_plan(&self, quality: Quality) -> Option<FramePlan> {
        self.project.active().map(|s| ve_render::evaluate(s, self.playhead(), quality))
    }

    // ---- events ----------------------------------------------------------

    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }
}
