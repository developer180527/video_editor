//! The editor UI: libgui panels over the engine client.
//!
//! The UI reads [`Published`] state and sends [`Command`]s; it never changes
//! the project itself, never touches files, and never asks which OS it is on.
//! Anything only the host can do (show a file dialog) goes out as a
//! [`HostRequest`]. A touch-first iPad layout is a different arrangement of
//! the same panels.
//!
//! The look is ported from the `libgui_cut` mock-up.

mod color_panel;
mod dock;
mod editing;
mod effects;
mod features;
mod graph;
mod graphics;
mod menu;
mod mixer_panel;
mod program;
mod scopes_panel;
mod settings;
mod source;
mod project;
pub mod theme;
mod timeline;
mod topbar;
#[allow(dead_code)] // the icon set is a library; not every icon is used yet
mod widgets;

use std::collections::{HashMap, HashSet};

use libgui::*;
use ve_engine::{Command, EngineClient, Event, Published, Snapshot, Viewer};
use ve_model::*;
use ve_render::Compositor;
use ve_time::{Rate, Time, Timecode};

pub use dock::Tab;
pub use features::Dialog;
pub use menu::{Action, Entry, Menu, MenuItem};
pub use settings::Settings;
pub use theme::theme;

/// Payload kind the shell uses for files dragged in from the OS, carrying
/// `Vec<PathBuf>`. (Matches `libgui_winit::FILES`.)
pub const FILES_PAYLOAD: &str = "files";
/// Payload kind for an asset dragged out of the project bin, carrying `AssetId`.
pub const ASSET_PAYLOAD: &str = "asset";

/// Things only the host can do, asked for by the UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostRequest {
    /// Show the platform's file picker and import what is chosen.
    ImportMedia,
    SaveProjectAs,
    OpenProject,
    /// Ask where to export; answer with [`EditorUi::start_export`].
    ExportAs { default_name: String, extension: String },
    /// Pick a file to be `asset`'s proxy; answer with [`EditorUi::attach_proxy`].
    AttachProxy(AssetId),
    /// Pick the file `asset` now lives at; answer with [`EditorUi::relink`].
    RelinkMedia(AssetId),
    /// Store [`EditorUi::settings_toml`]: the settings changed.
    SaveSettings,
    /// Store [`EditorUi::layout_toml`] and the windows' frames, to put back
    /// at the next start with [`EditorUi::restore_layout`].
    SaveLayout,
}

/// The timeline tools, in the order of the tool column.
/// How the host frames a window, so the editor can draw a title bar of its
/// own in place of the OS's.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct WindowFrame {
    pub controls: WindowControls,
    /// The menus are in the system menu bar: don't draw them in the window.
    pub system_menu: bool,
}

/// Who draws a window's minimize / maximize / close buttons, and where.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum WindowControls {
    /// The OS frames the window (a tablet, tests): nothing to draw.
    #[default]
    Os,
    /// The OS draws its buttons over the left end of the editor's bar: keep
    /// `inset` logical px clear (0 when they are hidden, in full screen).
    Leading { inset: f32 },
    /// The editor draws them at the right end of its bar.
    Drawn { maximized: bool },
}

/// A window button the editor drew was pressed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    Minimize,
    ToggleMaximize,
    Close,
}

/// Where the drag of a knob, fader or wheel began: the pointer, and the
/// control's value then. Drags are measured from here, never by adding up
/// per-frame movement: the press frame's movement (the pointer travelling
/// to the control) would otherwise nudge a control that was only clicked.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DragStart {
    pos: Vec2,
    value: [f32; 2],
    moved: bool,
}

/// How far (logical px) the pointer must move before a press becomes a drag.
const DRAG_DEAD_ZONE: f32 = 3.0;

/// For a control's response `r` and its current `value`: while it is being
/// dragged past the dead zone, the value at the press and the pointer's
/// offset from where it was pressed.
pub(crate) fn drag_from(start: &mut Option<DragStart>, r: &Response, value: [f32; 2]) -> Option<([f32; 2], Vec2)> {
    if r.pressed {
        *start = Some(DragStart { pos: r.mouse_pos, value, moved: false });
    }
    if !r.active {
        if r.released {
            *start = None;
        }
        return None;
    }
    let s = start.as_mut()?;
    let d = Vec2::new(r.mouse_pos.x - s.pos.x, r.mouse_pos.y - s.pos.y);
    if !s.moved && d.x.abs().max(d.y.abs()) < DRAG_DEAD_ZONE {
        return None;
    }
    s.moved = true;
    Some((s.value, d))
}

/// The tab bar's height in a torn-off window, where it is the title bar.
pub const FLOATING_TAB_HEIGHT: f32 = 36.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tool {
    Select,
    TrackSelect,
    Ripple,
    Rolling,
    Razor,
    Slip,
    Pen,
    Rect,
    Hand,
    /// Click the Program monitor to make a title there.
    Type,
}

/// A parameter being dragged in Effect Controls, shown before it is committed.
#[derive(Clone, Debug, PartialEq)]
pub struct ParamEdit {
    pub clip: ClipId,
    pub effect: EffectId,
    pub param: String,
    pub value: Value,
}

/// UI-only state: nothing here is part of the document.
pub struct View {
    pub selected_asset: Option<AssetId>,
    /// Selected clips; the first is the one Effect Controls shows.
    pub selection: Vec<ClipId>,
    pub tool: Tool,
    /// Timeline zoom, px per second, and scroll.
    pub pps: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub drag: Option<timeline::Drag>,
    pub snap: bool,
    pub linked: bool,
    /// Tracks targeted for edits (the highlighted V1 / A1 patches).
    pub targeted: HashSet<TrackId>,
    pub track_heights: HashMap<TrackId, f32>,
    pub workspace: usize,
    pub fit: usize,
    pub quality: usize,
    pub thumb: f32,
    pub search: String,
    /// The Effects panel's filter.
    pub effects_search: String,
    /// Effect Controls twirls, closed when present.
    pub closed: HashSet<EffectId>,
    pub param_edit: Option<ParamEdit>,
    /// Play proxies where assets have them.
    pub proxies: bool,
    /// The dialog in front, if any.
    pub dialog: Option<features::Dialog>,
    /// A selected transition: the clip that owns it, and which edge.
    pub selected_transition: Option<(ClipId, ve_engine::Edge)>,
    pub selected_marker: Option<MarkerId>,
    /// A transition's duration, in frames, while it is being dragged.
    pub transition_frames: Option<f32>,
    /// The undo step a drag of a transition's parameter is building.
    pub transition_gesture: Option<u64>,
    /// Effect parameters whose keyframe graph is open.
    pub graphs: HashSet<(EffectId, String)>,
    /// The selected key in a graph, and what a graph drag holds.
    pub(crate) graph_key: Option<graph::GraphKey>,
    pub(crate) graph_drag: Option<graph::Grab>,
    /// The graph's value scale, held while dragging.
    pub(crate) graph_scale: Option<(f64, f64)>,
    /// Essential Graphics: 0 Browse, 1 Edit; the selected template.
    pub graphics_tab: usize,
    pub template: usize,
    /// Which scope the Scopes panel shows.
    pub scope: ve_render::scopes::ScopeKind,
    /// A fader or pan knob being dragged: the track and its live values.
    pub track_drag: Option<(TrackId, f32, f32)>,
}

pub struct EditorUi {
    pub engine: EngineClient,
    pub view: View,
    dock: DockState<Tab>,
    compositor: Option<Compositor>,
    /// The platform's importer for frames left in GPU memory, if any.
    importer: Option<std::sync::Arc<dyn ve_render::TextureImporter>>,
    /// The monitor shows a frame still being decoded: keep drawing.
    catching_up: bool,
    /// Meter levels with fall-off, dBFS.
    pub(crate) meter_db: [f32; 2],
    /// The GPU, kept for exports.
    gpu: Option<(wgpu::Device, wgpu::Queue)>,
    pub(crate) export: Option<std::sync::Arc<ve_engine::ExportState>>,
    pub(crate) export_preset: usize,
    pub(crate) show_export: bool,
    /// Thumbnail textures, by (asset, second).
    thumbs: HashMap<(AssetId, i64), (wgpu::Texture, TextureId)>,
    thumb_uploads: Vec<((AssetId, i64), std::sync::Arc<ve_engine::Thumb>)>,
    monitor: Option<TextureId>,
    /// The monitor's current picture, to point other windows' renderers at.
    monitor_view: Option<wgpu::TextureView>,
    /// Per torn-off window: the thumbnails its renderer has been given.
    shared: HashMap<SurfaceId, HashSet<TextureId>>,
    /// How the window being built is framed.
    frame: WindowFrame,
    window_actions: Vec<(SurfaceId, WindowAction)>,
    tab_height: f32,
    settings: Settings,
    scopes: Option<ve_render::scopes::Scopes>,
    /// The scope image, as the UI shows it, and its view (for other windows).
    scope_tex: Option<TextureId>,
    scope_view: Option<wgpu::TextureView>,
    /// A Scopes panel was drawn: measure the next frame.
    pub(crate) scopes_wanted: bool,
    /// Per-track meters (dB, falling smoothly) and the master's loudness.
    pub(crate) track_db: HashMap<TrackId, [f32; 2]>,
    pub(crate) loudness: ve_engine::Loudness,
    /// Numbers the drags of sliders, so each is one undo step.
    gesture: u64,
    /// The gesture of the slider or wheel being dragged.
    pub(crate) drag_gesture: u64,
    /// Where the knob, fader or wheel being dragged was pressed.
    pub(crate) drag_start: Option<DragStart>,
    /// A grade effect added but maybe not in the snapshot yet.
    pub(crate) pending_grade: Option<(ClipId, EffectId)>,
    /// The engine's state as of this frame.
    st: Published,
    playhead: Time,
    /// The Source monitor's playhead.
    pub(crate) source_playhead: Time,
    source_comp: Option<Compositor>,
    source_tex: Option<TextureId>,
    source_view: Option<wgpu::TextureView>,
    /// A Source monitor panel was drawn: render its picture next frame.
    pub(crate) source_wanted: bool,
    /// Where to put the program playhead once the sequence is long enough
    /// (after an edit the engine has not applied yet).
    /// Dropped after a second: the edit failed.
    pub(crate) seek_after_edit: Option<(Time, std::time::Instant)>,
    /// Where the Source monitor goes once this asset has opened in it
    /// (Match Frame): the engine opens it at its in point first.
    pub(crate) source_seek_on_open: Option<(AssetId, Time)>,
    /// The marks last sent, until the engine publishes them.
    pub(crate) pending_marks: Option<(ve_engine::MarksOwner, Marks, std::time::Instant)>,
    /// Where the last keyframe graph was drawn (tests aim at it).
    pub(crate) graph_rect: Option<Rect>,
    /// Where the Program monitor's picture was drawn (tests aim at it).
    pub(crate) monitor_rect: Option<Rect>,
    /// An Essential Graphics panel was drawn: it wants template pictures.
    pub(crate) graphics_wanted: bool,
    /// Template pictures, by template index.
    pub(crate) template_tex: Vec<Option<(wgpu::Texture, TextureId)>>,
    /// Typing in a title's text: one undo step per stretch of typing.
    pub(crate) text_gesture: Option<(ClipId, u64)>,
    /// A colour being picked: one undo step until the picker closes.
    pub(crate) color_gesture: Option<(&'static str, u64)>,
    /// The Source picture is still decoding: keep drawing.
    source_catching_up: bool,
    /// Refusals from this frame, shown as toasts on the next.
    errors: Vec<String>,
    requests: Vec<HostRequest>,
    #[allow(dead_code)] // the tablet arrangement is chosen at construction; kept for touch-specific input next
    touch: bool,
}

impl EditorUi {
    /// `touch` picks the tablet arrangement (iPad).
    pub fn new(engine: EngineClient, touch: bool) -> Self {
        let st = engine.published();
        let mut dock = dock::initial(touch);
        if touch {
            dock.config.floating_mode = FloatingMode::InApp;
        }
        EditorUi {
            engine,
            view: View {
                selected_asset: None,
                selection: Vec::new(),
                tool: Tool::Select,
                pps: 78.0,
                scroll_x: 0.0,
                scroll_y: 0.0,
                drag: None,
                snap: true,
                linked: true,
                targeted: HashSet::new(),
                track_heights: HashMap::new(),
                workspace: 1,
                fit: 0,
                quality: 1,
                thumb: 0.7,
                search: String::new(),
                effects_search: String::new(),
                closed: HashSet::new(),
                param_edit: None,
                proxies: false,
                dialog: None,
                selected_transition: None,
                selected_marker: None,
                transition_frames: None,
                transition_gesture: None,
                graphs: HashSet::new(),
                graph_key: None,
                graph_drag: None,
                graph_scale: None,
                graphics_tab: 0,
                template: 0,
                scope: Default::default(),
                track_drag: None,
            },
            dock,
            compositor: None,
            importer: None,
            catching_up: false,
            meter_db: [-96.0; 2],
            gpu: None,
            export: None,
            export_preset: 0,
            show_export: false,
            thumbs: HashMap::new(),
            thumb_uploads: Vec::new(),
            monitor: None,
            monitor_view: None,
            shared: HashMap::new(),
            frame: WindowFrame::default(),
            window_actions: Vec::new(),
            tab_height: 24.0,
            settings: Settings::default(),
            scopes: None,
            scope_tex: None,
            scope_view: None,
            scopes_wanted: false,
            track_db: HashMap::new(),
            loudness: ve_engine::Loudness::default(),
            gesture: 0,
            drag_gesture: 0,
            drag_start: None,
            pending_grade: None,
            st,
            playhead: Time::ZERO,
            source_playhead: Time::ZERO,
            source_comp: None,
            source_tex: None,
            source_view: None,
            source_wanted: false,
            source_catching_up: false,
            seek_after_edit: None,
            source_seek_on_open: None,
            pending_marks: None,
            graph_rect: None,
            monitor_rect: None,
            graphics_wanted: false,
            template_tex: Vec::new(),
            text_gesture: None,
            color_gesture: None,
            errors: Vec::new(),
            requests: Vec::new(),
            touch,
        }
    }

    pub fn dock_mut(&mut self) -> &mut DockState<Tab> {
        &mut self.dock
    }

    pub fn dock(&self) -> &DockState<Tab> {
        &self.dock
    }

    /// A panel's title, as its tab and a torn-off window show it.
    pub fn tab_title(&mut self, tab: Tab) -> String {
        dock::Viewer { app: self }.title_of(tab)
    }

    /// The project's name, for a default file name.
    pub fn project_name(&self) -> String {
        self.st.snapshot.name.clone()
    }

    /// What the host should do on the UI's behalf (drained each call).
    pub fn take_requests(&mut self) -> Vec<HostRequest> {
        std::mem::take(&mut self.requests)
    }

    /// Hand hardware-decoded frames to the compositor as they are, through
    /// the platform's `importer` (frames are copied through memory without
    /// one). Call before the first frame is drawn.
    pub fn set_importer(&mut self, importer: Option<std::sync::Arc<dyn ve_render::TextureImporter>>) {
        self.engine.video().set_gpu_frames(importer.is_some());
        if let Some(c) = &mut self.compositor {
            c.set_importer(importer.clone());
        }
        self.importer = importer;
    }

    /// Render the program monitor and hand its texture (and new thumbnails)
    /// to the main window's UI renderer.
    pub fn prepare_gpu(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut libgui_wgpu::Renderer) {
        // Resolution from the quality menu; proxies from their toggle.
        let quality = ve_render::Quality { scale: [1.0, 0.5, 0.25][self.view.quality.min(2)], use_proxies: self.view.proxies };
        if self.gpu.is_none() {
            self.gpu = Some((device.clone(), queue.clone()));
        }
        // Template pictures, once, when a graphics panel first wants them.
        if self.graphics_wanted && self.template_tex.is_empty() {
            for t in graphics::templates() {
                let tex = graphics::template_picture(self, &t).map(|(w, h, rgba)| upload_rgba(device, queue, renderer, w, h, &rgba));
                self.template_tex.push(tex);
            }
        }
        for (key, img) in self.thumb_uploads.drain(..) {
            if self.thumbs.contains_key(&key) {
                continue;
            }
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("thumb"),
                size: wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &img.rgba,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(img.width * 4), rows_per_image: Some(img.height) },
                wgpu::Extent3d { width: img.width, height: img.height, depth_or_array_layers: 1 },
            );
            let id = renderer.register_texture(&tex.create_view(&Default::default()));
            self.thumbs.insert(key, (tex, id));
        }
        // The Source monitor's picture, while a Source panel shows it.
        self.source_catching_up = false;
        if std::mem::take(&mut self.source_wanted) {
            if let Some(frame) = self.engine.frame_of(Viewer::Source, quality) {
                let importer = &self.importer;
                let comp = self.source_comp.get_or_insert_with(|| {
                    let mut c = Compositor::new(device);
                    c.set_importer(importer.clone());
                    c
                });
                comp.render(device, queue, &frame.plan, &frame.layers, frame.seq_size, frame.space);
                self.source_catching_up = !frame.complete;
                self.errors.append(&mut comp.errors);
                if let Some(tex) = comp.output() {
                    let view = tex.create_view(&Default::default());
                    match self.source_tex {
                        Some(id) => renderer.update_texture(id, &view),
                        None => self.source_tex = Some(renderer.register_texture(&view)),
                    }
                    self.source_view = Some(view);
                }
            }
        }
        let Some(frame) = self.engine.frame(quality) else { return };
        let importer = &self.importer;
        let comp = self.compositor.get_or_insert_with(|| {
            let mut c = Compositor::new(device);
            c.set_importer(importer.clone());
            c
        });
        comp.render(device, queue, &frame.plan, &frame.layers, frame.seq_size, frame.space);
        self.catching_up = !frame.complete;
        self.errors.append(&mut comp.errors);
        if let Some(tex) = comp.output() {
            let view = tex.create_view(&Default::default());
            match self.monitor {
                Some(id) => renderer.update_texture(id, &view),
                None => self.monitor = Some(renderer.register_texture(&view)),
            }
            self.monitor_view = Some(view);
            // Scopes measure what the monitor shows, when a panel shows them.
            if std::mem::take(&mut self.scopes_wanted) {
                let scopes = self.scopes.get_or_insert_with(|| ve_render::scopes::Scopes::new(device));
                scopes.render(device, queue, tex, self.view.scope);
                if let Some(out) = scopes.output() {
                    let view = out.create_view(&Default::default());
                    match self.scope_tex {
                        Some(id) => renderer.update_texture(id, &view),
                        None => self.scope_tex = Some(renderer.register_texture(&view)),
                    }
                    self.scope_view = Some(view);
                }
            }
        }
    }

    pub fn animating(&self) -> bool {
        self.engine.is_playing()
            || self.st.busy.is_some()
            || self.catching_up
            || self.source_catching_up
            || self.meter_db.iter().any(|d| *d > -95.0)
            || self.track_db.values().flatten().any(|d| *d > -95.0)
            || self.export.as_ref().is_some_and(|e| e.result().is_none())
            || !self.thumb_uploads.is_empty()
    }

    /// The thumbnail of `asset` near source time `t`, once made and uploaded.
    pub(crate) fn thumb_tex(&mut self, asset: &Asset, t: Time) -> Option<TextureId> {
        let (key, img) = self.engine.stills().thumb(asset.id, &asset.media, t)?;
        if let Some((_, id)) = self.thumbs.get(&key) {
            return Some(*id);
        }
        if !self.thumb_uploads.iter().any(|(k, _)| *k == key) {
            self.thumb_uploads.push((key, img));
        }
        None
    }

    /// Waveform peaks of `asset`'s `stream`-th audio stream (100 per
    /// second), once computed.
    pub(crate) fn peaks(&self, asset: &Asset, stream: usize) -> Option<std::sync::Arc<Vec<f32>>> {
        self.engine.stills().peaks(asset.id, &asset.media, stream)
    }

    /// The host's answer to [`HostRequest::AttachProxy`].
    pub fn attach_proxy(&mut self, asset: AssetId, path: &std::path::Path) {
        self.engine.attach_proxy(asset, path.to_string_lossy().into_owned());
    }

    /// The host's answer to [`HostRequest::RelinkMedia`].
    pub fn relink(&mut self, asset: AssetId, path: &std::path::Path) {
        self.engine.relink(asset, path.to_string_lossy().into_owned());
    }

    /// Export to `path` with the chosen preset (after the host's save dialog).
    pub fn start_export(&mut self, path: &std::path::Path) {
        let Some(gpu) = self.gpu.clone() else {
            self.errors.push("Export needs the GPU, which is not ready yet.".into());
            return;
        };
        let preset = ve_engine::ExportPreset::ALL[self.export_preset.min(2)];
        let mut p = path.to_path_buf();
        if p.extension().is_none() {
            p.set_extension(preset.extension());
        }
        self.export = Some(self.engine.export(gpu, preset, MediaRef(format!("file:{}", p.display())), self.importer.clone()));
    }

    /// The export dialog, and the end of a finished export.
    fn export_ui(&mut self, ui: &mut Ui) {
        if let Some(job) = &self.export {
            if let Some(r) = job.result() {
                match r {
                    Ok(()) => {
                        ui.toast(Toast::success(format!("Exported {}", job.name)));
                    }
                    Err(e) if e == "cancelled" => {
                        ui.toast(Toast::info("Export cancelled"));
                    }
                    Err(e) => {
                        ui.toast(Toast::error(format!("Export failed: {e}")));
                    }
                }
                self.export = None;
            }
        }
        if !self.show_export {
            return;
        }
        let seq = self.snap().active().cloned();
        let opts = ModalOptions { width: 420.0, ..Default::default() };
        let presets: Vec<&str> = ve_engine::ExportPreset::ALL.iter().map(|p| p.label()).collect();
        let mut preset = self.export_preset;
        let r = ui.modal("export", "Export Settings", &opts, |ui| {
            match &seq {
                Some(s) => {
                    let f = &s.format;
                    ui.label(&format!("{}: {}×{} at {:.3} fps, {}", s.name, f.width, f.height, f.rate.as_f64(), Timecode::from_time(s.duration(), f.rate, true)));
                }
                None => ui.label_muted("No sequence."),
            }
            ui.space(6.0);
            ui.combo("Format", &mut preset, &presets);
            ui.space(4.0);
            ui.label_muted("Video is encoded in Rec.709; audio as AAC, 48 kHz stereo.");
            ui.space(10.0);
            ui.row(|ui| {
                ui.flex();
                (ui.button("Cancel").clicked, ui.button_primary("Export…").clicked)
            })
        });
        self.export_preset = preset;
        let (cancel, go) = r.inner;
        if cancel || r.cancelled || r.clicked_outside {
            self.show_export = false;
        }
        if go || r.submitted {
            self.show_export = false;
            let p = ve_engine::ExportPreset::ALL[preset];
            let empty = seq.as_ref().is_none_or(|s| s.duration() == Time::ZERO);
            if empty {
                self.errors.push("Nothing to export: the sequence is empty.".into());
            } else {
                self.requests.push(HostRequest::ExportAs {
                    default_name: format!("{}.{}", self.snap().name, p.extension()),
                    extension: p.extension().into(),
                });
            }
        }
    }

    /// Peak meters with a 20 dB/s fall-off, as meters move.
    fn update_meters(&mut self, dt: f32) {
        let peaks = self.st.meters.as_ref().map(|m| m.peaks()).unwrap_or([0.0; 2]);
        let playing = self.engine.is_playing();
        let fall = |db: &mut f32, p: f32| {
            let now = if playing && p > 0.0 { 20.0 * p.log10() } else { -96.0 };
            *db = now.max(*db - 20.0 * dt).max(-96.0);
        };
        for (db, p) in self.meter_db.iter_mut().zip(peaks) {
            fall(db, p);
        }
        // Per track, and loudness, as of what is being heard.
        let levels = self.st.meters.as_ref().map(|m| m.levels()).unwrap_or_default();
        self.loudness = levels.loudness;
        for (id, p) in &levels.tracks {
            let db = self.track_db.entry(*id).or_insert([-96.0; 2]);
            fall(&mut db[0], p[0]);
            fall(&mut db[1], p[1]);
        }
        for (id, db) in self.track_db.iter_mut() {
            if !levels.tracks.iter().any(|(t, _)| t == id) {
                fall(&mut db[0], 0.0);
                fall(&mut db[1], 0.0);
            }
        }
    }

    /// A new drag of a slider: its commands share this key, one undo step.
    pub(crate) fn next_gesture(&mut self) -> u64 {
        self.gesture += 1;
        self.gesture
    }

    fn snap(&self) -> &Snapshot {
        &self.st.snapshot
    }

    fn rate(&self) -> Rate {
        self.snap().active().map(|s| s.format.rate).unwrap_or(Rate::FPS_24)
    }

    fn timecode(&self, t: Time) -> String {
        Timecode::from_time(t, self.rate(), true).to_string()
    }

    /// Send a command. Refusals come back from the engine as events.
    fn run(&mut self, cmd: Command) {
        self.engine.execute(cmd);
    }

    /// Run an edit tool's result, or show why it could not be built.
    fn run_edit(&mut self, r: Result<Command, ve_engine::CommandError>) {
        match r {
            Ok(c) => self.run(c),
            Err(e) => self.errors.push(e.to_string()),
        }
    }

    /// Move the Program monitor's playhead (it becomes the active monitor).
    fn seek(&mut self, t: Time) {
        let t = t.max(Time::ZERO);
        self.engine.set_viewer(Viewer::Program);
        self.engine.seek(t);
        self.playhead = t;
    }

    /// Move the Source monitor's playhead (it becomes the active monitor).
    pub(crate) fn seek_source(&mut self, t: Time) {
        if self.engine.source().is_none() {
            return;
        }
        let t = t.max(Time::ZERO);
        self.engine.set_viewer(Viewer::Source);
        self.engine.seek(t);
        self.source_playhead = t;
    }

    /// The active monitor: what play, J/K/L, the arrows and marks act on.
    pub(crate) fn viewer(&self) -> Viewer {
        self.engine.viewer()
    }

    /// The active monitor's playhead, frame rate and length.
    pub(crate) fn active_position(&self) -> (Time, Rate, Time) {
        match (self.viewer(), self.engine.source()) {
            (Viewer::Source, Some(s)) => (self.source_playhead, s.sequence.format.rate, s.duration()),
            _ => (self.playhead, self.rate(), self.snap().active().map(|s| s.duration()).unwrap_or(Time::ZERO)),
        }
    }

    fn seek_active(&mut self, t: Time) {
        match self.viewer() {
            Viewer::Source => self.seek_source(t),
            Viewer::Program => self.seek(t),
        }
    }

    /// Step the active monitor `frames` frames, stopped.
    fn nudge(&mut self, frames: i64) {
        self.engine.stop();
        let (t, r, _) = self.active_position();
        self.seek_active(r.frame_to_time(t.to_frame(r) + frames));
    }

    /// J and L, as editors shuttle: each press of the same key doubles the
    /// speed (1×, 2×, 4×, 8×); the other key slows down, then reverses.
    pub(crate) fn shuttle(&mut self, forward: bool) {
        let dir = if forward { 1.0 } else { -1.0 };
        let rate = match self.engine.published().transport.state() {
            ve_engine::State::Playing { rate } => rate,
            _ => 0.0,
        };
        let next = if rate * dir > 0.0 {
            (rate * 2.0).clamp(-8.0, 8.0)
        } else if rate.abs() > 1.0 {
            rate / 2.0
        } else {
            dir
        };
        self.engine.play(next);
    }

    /// Start of the next frame at or after `t`, shifted by `frames`.
    fn step(&self, frames: i64) -> Time {
        let r = self.rate();
        r.frame_to_time(self.playhead.to_frame(r) + frames)
    }

    /// Give a torn-off window's renderer the textures the main window's has
    /// (the monitor, thumbnails), under the same ids, so panels show the same
    /// pictures wherever they are. Call before building that window's UI.
    pub fn share_textures(&mut self, surface: SurfaceId, renderer: &mut libgui_wgpu::Renderer) {
        if surface == SurfaceId::MAIN {
            return;
        }
        let bound = self.shared.entry(surface).or_default();
        // The monitor's picture changes underneath its id: point at it each frame.
        if let (Some(id), Some(view)) = (self.monitor, &self.monitor_view) {
            renderer.update_texture(id, view);
        }
        if let (Some(id), Some(view)) = (self.scope_tex, &self.scope_view) {
            renderer.update_texture(id, view);
        }
        if let (Some(id), Some(view)) = (self.source_tex, &self.source_view) {
            renderer.update_texture(id, view);
        }
        for (tex, id) in self.thumbs.values().chain(self.template_tex.iter().flatten()) {
            if bound.insert(*id) {
                renderer.update_texture(*id, &tex.create_view(&Default::default()));
            }
        }
    }

    /// Where the last keyframe graph lane was drawn, for tests that drive it.
    #[doc(hidden)]
    pub fn graph_rect(&self) -> Option<Rect> {
        self.graph_rect
    }

    /// Each title template's name and picture (320×180 RGBA), as the
    /// Essential Graphics cards show them; for tests.
    #[doc(hidden)]
    pub fn template_pictures(&self) -> Vec<(String, u32, u32, Vec<u8>)> {
        graphics::templates().iter().filter_map(|t| graphics::template_picture(self, t).map(|(w, h, px)| (t.name.to_string(), w, h, px))).collect()
    }

    /// Where the Program monitor's picture was drawn, for tests that drive it.
    #[doc(hidden)]
    pub fn monitor_rect(&self) -> Option<Rect> {
        self.monitor_rect
    }

    /// Show the user something the host could not do (a toast).
    pub fn report_error(&mut self, message: String) {
        self.errors.push(message);
    }

    /// The dock layout (panes, tabs, torn-off windows' sizes) as the host
    /// stores it.
    pub fn layout_toml(&mut self) -> Option<String> {
        let dock = std::mem::replace(&mut self.dock, DockState::new());
        let text = dock.layout(&dock::Viewer { app: self }).to_toml().ok();
        self.dock = dock;
        text
    }

    /// Put back a layout from [`EditorUi::layout_toml`]. Panels it never
    /// mentioned (added since) join the main window, apart from Settings,
    /// which opens on request. False if the text is not a layout.
    pub fn restore_layout(&mut self, text: &str) -> bool {
        let Ok(saved) = DockLayout::from_toml(text) else { return false };
        let Ok(report) = self.dock.restore(&saved, Tab::from_key) else { return false };
        for key in report.missing_from(Tab::ALL.iter().map(|t| t.key())) {
            if let Some(tab) = Tab::from_key(key).filter(|t| *t != Tab::Settings) {
                self.dock.add_tab(SurfaceId::MAIN, tab);
            }
        }
        true
    }

    /// Open `tab` in a window of its own (a floating panel on a tablet),
    /// or bring it forward where it already is.
    pub fn open_floating(&mut self, tab: Tab, size: Vec2) {
        if let Some(at) = self.dock.find_tab(|t| *t == tab) {
            self.dock.focus_tab(at);
            return;
        }
        let dock = std::mem::replace(&mut self.dock, DockState::new());
        let mut layout = dock.layout(&dock::Viewer { app: self });
        self.dock = dock;
        layout.surfaces.push(SurfaceLayout {
            floating: true,
            size,
            rect: Rect::new(120.0, 90.0, size.x, size.y),
            root: Some(NodeLayout::Leaf { tabs: vec![tab.key()], active: 0, titles: Vec::new() }),
        });
        let _ = self.dock.restore(&layout, Tab::from_key);
    }

    /// The height of the strip at the top of a window that is its title bar:
    /// press empty space there to move the window.
    pub fn title_strip(&self, surface: SurfaceId) -> f32 {
        if surface == SurfaceId::MAIN {
            topbar::HEIGHT
        } else {
            self.tab_height
        }
    }

    /// The window buttons the editor drew that were pressed (drained).
    pub fn take_window_actions(&mut self) -> Vec<(SurfaceId, WindowAction)> {
        std::mem::take(&mut self.window_actions)
    }

    /// One frame for one dock surface (the main window, or a torn-off panel),
    /// framed by the OS.
    pub fn ui_for(&mut self, ui: &mut Ui, surface: SurfaceId) {
        self.ui_framed(ui, surface, WindowFrame::default());
    }

    /// One frame for one dock surface, in a window framed as `frame` says:
    /// the editor draws the title bar (or keeps clear of the OS's buttons).
    pub fn ui_framed(&mut self, ui: &mut Ui, surface: SurfaceId, frame: WindowFrame) {
        self.frame = frame;
        // A torn-off window's tab bar is its title bar: as tall as one. Each
        // window has its own `Ui`, so docked tabs keep the theme's height.
        if surface != SurfaceId::MAIN && frame.controls != WindowControls::Os {
            ui.theme.tab.height = FLOATING_TAB_HEIGHT;
        }
        self.tab_height = ui.theme.tab.height;
        if surface != SurfaceId::MAIN {
            self.shared.retain(|id, _| self.dock.surface(*id).is_some());
        }
        if surface == SurfaceId::MAIN {
            self.st = self.engine.published();
            self.playhead = self.engine.playhead_of(Viewer::Program);
            self.source_playhead = self.engine.playhead_of(Viewer::Source);
            if let Some((asset, t)) = self.source_seek_on_open {
                if self.engine.source().is_some_and(|s| s.asset == asset) {
                    self.source_seek_on_open = None;
                    self.seek_source(t);
                }
            }
            if let Some((t, since)) = self.seek_after_edit {
                if self.snap().active().is_some_and(|s| s.duration() >= t) {
                    self.seek_after_edit = None;
                    self.seek(t);
                } else if since.elapsed().as_secs_f32() > 1.0 {
                    self.seek_after_edit = None;
                }
            }
            for e in self.engine.drain_events() {
                if let Event::Error(msg) = e {
                    self.errors.push(msg);
                }
            }
            for msg in self.errors.drain(..) {
                ui.toast(Toast::error(msg));
            }
            self.default_targets();
            self.update_meters(ui.input().dt.min(0.1));
        }
        let t = ui.theme.clone();
        let root = Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0));
        ui.container(root, Frame { fill: t.palette.bg_app, clip: true, ..Frame::none() }, |ui| {
            if surface == SurfaceId::MAIN {
                topbar::bar(ui, self);
            }
            // The dock is borrowed apart from the rest of `self` through the viewer.
            let mut dock = std::mem::replace(&mut self.dock, DockState::new());
            // A torn-off window's tab bar is its title bar: its tabs keep
            // clear of the OS's window buttons.
            let padding = dock.config.tab_bar_padding;
            if let (true, WindowControls::Leading { inset }) = (surface != SurfaceId::MAIN, frame.controls) {
                dock.config.tab_bar_padding = inset.max(padding);
            }
            let mut viewer = dock::Viewer { app: self };
            dock.show(ui, surface, &mut viewer);
            dock.config.tab_bar_padding = padding;
            self.dock = dock;
        });
        if surface != SurfaceId::MAIN {
            if let WindowControls::Drawn { maximized } = frame.controls {
                let w = topbar::CONTROLS_W;
                let rect = Rect::new(ui.screen_size().x - w, 0.0, w, self.tab_height);
                ui.container_at(Id::new(("window-controls", surface.0)), rect, Frame::none(), |ui| {
                    ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Grow(1.0)), Frame::none(), |ui| {
                        for a in topbar::window_controls(ui, maximized, self.tab_height) {
                            self.window_actions.push((surface, a));
                        }
                    });
                });
            }
        }
        if surface == SurfaceId::MAIN {
            self.export_ui(ui);
            self.dialogs(ui);
        }
        // Keys go to the focused window, so every window takes shortcuts
        // (after the dialogs: an open one holds them back).
        self.shortcuts(ui);
        if surface == SurfaceId::MAIN {
            ui.drag_ghost();
            ui.show_toasts();
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        self.ui_for(ui, SurfaceId::MAIN);
    }

    /// The first video and first audio track are targeted until the user says
    /// otherwise.
    fn default_targets(&mut self) {
        let Some(seq) = self.snap().active().cloned() else { return };
        self.view.targeted.retain(|id| seq.track(*id).is_some());
        if self.view.targeted.is_empty() {
            for kind in [TrackKind::Video, TrackKind::Audio] {
                if let Some(t) = seq.tracks.iter().find(|t| t.kind == kind) {
                    self.view.targeted.insert(t.id);
                }
            }
        }
    }

    fn shortcuts(&mut self, ui: &mut Ui) {
        if ui.any_popup_open() || ui.any_modal_open() {
            return;
        }
        // Cmd on Apple keyboards, Ctrl elsewhere: accept both.
        let cmd = |ui: &mut Ui, k: Key| ui.consume_shortcut(Shortcut::plain(k).logo()) || ui.consume_shortcut(Shortcut::plain(k).ctrl());
        let cmd_shift =
            |ui: &mut Ui, k: Key| ui.consume_shortcut(Shortcut::plain(k).logo().shift()) || ui.consume_shortcut(Shortcut::plain(k).ctrl().shift());
        let key = |ui: &mut Ui, k: Key| ui.consume_shortcut(Shortcut::plain(k));

        if cmd_shift(ui, Key::Z) {
            self.pending_marks = None;
            self.engine.redo();
        } else if cmd(ui, Key::Z) {
            self.pending_marks = None;
            self.engine.undo();
        }
        if cmd(ui, Key::I) {
            self.requests.push(HostRequest::ImportMedia);
        }
        if cmd_shift(ui, Key::S) {
            self.requests.push(HostRequest::SaveProjectAs);
        } else if cmd(ui, Key::S) {
            match self.st.file.clone() {
                Some(f) => self.engine.save_as(f),
                None => self.requests.push(HostRequest::SaveProjectAs),
            }
        }
        if cmd(ui, Key::M) {
            self.show_export = true;
        }
        if cmd(ui, Key::Comma) {
            self.open_floating(Tab::Settings, Vec2::new(560.0, 520.0));
        }
        if cmd(ui, Key::R) {
            self.open_speed_dialog();
        }
        if cmd_shift(ui, Key::D) {
            self.apply_default_transition(TrackKind::Audio);
        } else if cmd(ui, Key::D) {
            self.apply_default_transition(TrackKind::Video);
        }
        if ui.consume_shortcut(Shortcut::plain(Key::X).alt()) {
            self.clear_in_out();
        }
        if ui.consume_shortcut(Shortcut::plain(Key::M).alt().shift()) {
            self.go_to_marker(false);
        } else if ui.consume_shortcut(Shortcut::plain(Key::M).shift()) {
            self.go_to_marker(true);
        }
        if key(ui, Key::I) {
            self.mark_in();
        }
        if key(ui, Key::O) {
            self.mark_out();
        }
        if key(ui, Key::M) {
            self.add_marker();
        }
        // Multicam: 1–9 switch the angle from the playhead on.
        for (n, k) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9].into_iter().enumerate() {
            if key(ui, k) {
                self.switch_angle(n as u32 + 1);
            }
        }
        if cmd(ui, Key::O) {
            self.requests.push(HostRequest::OpenProject);
        }
        if cmd(ui, Key::K) {
            if let Some(seq) = self.snap().active().cloned() {
                let targets: Vec<TrackId> = self.view.targeted.iter().copied().collect();
                let r = ve_engine::edit::add_edit(self.snap(), seq.id, &targets, self.playhead);
                self.run_edit(r);
            }
        }
        // Transport: on the active monitor (Source or Program).
        if key(ui, Key::Space) {
            if self.engine.is_playing() {
                self.engine.stop();
            } else {
                self.engine.play(1.0);
            }
        }
        if key(ui, Key::K) {
            self.engine.stop();
        }
        // With K held, J and L step a frame; otherwise they shuttle.
        let k_held = ui.key_down(Key::K);
        if key(ui, Key::L) {
            if k_held {
                self.nudge(1);
            } else {
                self.shuttle(true);
            }
        }
        if key(ui, Key::J) {
            if k_held {
                self.nudge(-1);
            } else {
                self.shuttle(false);
            }
        }
        if key(ui, Key::ArrowLeft) {
            self.nudge(-1);
        }
        if key(ui, Key::ArrowRight) {
            self.nudge(1);
        }
        editing::shortcuts(self, ui);
        // Three-point editing from the Source monitor: , inserts, . overwrites.
        if key(ui, Key::Comma) {
            self.edit_from_source(true);
        }
        if key(ui, Key::Period) {
            self.edit_from_source(false);
        }
        if key(ui, Key::ArrowUp) || key(ui, Key::ArrowDown) {
            let up = ui.key_pressed(Key::ArrowUp);
            if let Some(seq) = self.snap().active() {
                let pts = ve_engine::edit::snap_points(seq, &[], Time::MAX);
                let next = if up {
                    pts.iter().rev().find(|&&t| t < self.playhead).copied()
                } else {
                    pts.iter().find(|&&t| t > self.playhead && t != Time::MAX).copied()
                };
                if let Some(t) = next {
                    self.seek(t);
                }
            }
        }
        if key(ui, Key::Home) {
            self.seek_active(Time::ZERO);
        }
        if key(ui, Key::End) {
            let (_, _, end) = self.active_position();
            self.seek_active(end);
        }
        let shift_delete = ui.consume_shortcut(Shortcut::plain(Key::Delete).shift()) || ui.consume_shortcut(Shortcut::plain(Key::Backspace).shift());
        let delete = key(ui, Key::Delete) || key(ui, Key::Backspace);
        // A selected graph key, transition or marker goes first; then clips.
        if delete {
            if let Some(g) = self.view.graph_key.clone() {
                let found = self.view.selection.iter().find_map(|id| {
                    let (_, _, c) = self.snap().find_clip(*id)?;
                    let e = c.effects.iter().find(|e| e.id == g.effect)?;
                    match e.params.get(&g.param)? {
                        Param::Animated(keys) => Some((c.id, keys.clone())),
                        _ => None,
                    }
                });
                if let Some((clip, keys)) = found {
                    graph::delete_key(self, clip, g.effect, &g.param, &keys, g.index);
                    return;
                }
                self.view.graph_key = None;
            }
            if let Some((clip, edge)) = self.view.selected_transition.take() {
                self.remove_transition(clip, edge);
                return;
            }
            if let Some(id) = self.view.selected_marker.take() {
                self.put_marker(id, None);
                return;
            }
        }
        if (shift_delete || delete) && !self.view.selection.is_empty() {
            let sel = std::mem::take(&mut self.view.selection);
            let r = if shift_delete {
                ve_engine::edit::ripple_delete(self.snap(), &sel, self.view.linked)
            } else {
                ve_engine::edit::lift(self.snap(), &sel, self.view.linked)
            };
            self.run_edit(r);
        }
        for (k, tool) in [
            (Key::V, Tool::Select),
            (Key::A, Tool::TrackSelect),
            (Key::B, Tool::Ripple),
            (Key::N, Tool::Rolling),
            (Key::C, Tool::Razor),
            (Key::Y, Tool::Slip),
            (Key::P, Tool::Pen),
            (Key::H, Tool::Hand),
            (Key::T, Tool::Type),
        ] {
            if key(ui, k) {
                self.view.tool = tool;
            }
        }
        if key(ui, Key::S) {
            self.view.snap = !self.view.snap;
        }
        if key(ui, Key::Equal) {
            self.view.pps = (self.view.pps * 1.25).min(1200.0);
        }
        if key(ui, Key::Minus) {
            self.view.pps = (self.view.pps / 1.25).max(4.0);
        }
    }

    /// Put `asset` on the targeted tracks at `at`: an overwrite edit, or an
    /// insert edit, video and audio linked — one undo step.
    /// Put `asset` into the sequence at `at`: its marked part (in to out),
    /// or all of it.
    fn place_asset(&mut self, asset: AssetId, at: Time, video_track: Option<TrackId>, insert: bool) {
        let Some(a) = self.snap().assets.get(&asset).cloned() else { return };
        let Some(info) = a.info.clone() else {
            self.errors.push(format!("{} has not been probed", a.name));
            return;
        };
        let media = if info.duration > Time::ZERO { info.duration } else { Time::from_seconds(5) };
        let start = a.marks.in_point.unwrap_or(Time::ZERO).clamp_to(Time::ZERO, media);
        let end = a.marks.out_point.unwrap_or(media).clamp_to(start, media);
        if end > start {
            self.place_range(asset, ve_time::TimeRange::new(start, end - start), at, video_track, insert);
        }
    }

    /// Put `range` of `asset`'s media into the sequence at `at`.
    fn place_range(&mut self, asset: AssetId, range: ve_time::TimeRange, at: Time, video_track: Option<TrackId>, insert: bool) {
        let Some(seq) = self.snap().active().cloned() else { return };
        let Some(a) = self.snap().assets.get(&asset).cloned() else { return };
        let duration = range.duration;
        let target = |kind: TrackKind, explicit: Option<TrackId>| {
            explicit
                .filter(|id| seq.track(*id).is_some_and(|(_, t)| t.kind == kind))
                .or_else(|| seq.tracks.iter().find(|t| t.kind == kind && self.view.targeted.contains(&t.id)).map(|t| t.id))
        };
        // Picture and every audio stream, linked; audio tracks added if the
        // file has more streams than the sequence has tracks.
        let (adds, mut items) = ve_engine::clips_for_asset(&self.st.plugins, &seq, &a, duration, target(TrackKind::Video, video_track), target(TrackKind::Audio, video_track));
        // From the range's start in the media.
        for (_, clip) in &mut items {
            std::sync::Arc::make_mut(clip).source_range = range;
        }
        let add_tracks = ve_engine::Command::Batch { label: String::new(), commands: adds.clone() };
        let with_tracks = match add_tracks.apply(self.snap()) {
            Ok(r) => r.project,
            Err(e) => {
                self.errors.push(e.to_string());
                return;
            }
        };
        let edit = if insert { ve_engine::edit::insert(&with_tracks, seq.id, at, &items) } else { ve_engine::edit::overwrite(&with_tracks, seq.id, at, &items) };
        // One undo step: the new tracks and the edit.
        let r = edit.map(|e| match adds.is_empty() {
            true => e,
            false => ve_engine::Command::Batch { label: e.label(), commands: adds.into_iter().chain([e]).collect() },
        });
        self.run_edit(r);
    }
}

/// An RGBA8 picture as a texture the UI renderer can draw.
fn upload_rgba(device: &wgpu::Device, queue: &wgpu::Queue, renderer: &mut libgui_wgpu::Renderer, w: u32, h: u32, rgba: &[u8]) -> (wgpu::Texture, TextureId) {
    let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("ui picture"),
        size,
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        rgba,
        wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(w * 4), rows_per_image: Some(h) },
        size,
    );
    let id = renderer.register_texture(&tex.create_view(&Default::default()));
    (tex, id)
}

#[cfg(test)]
mod drag_tests {
    use super::*;

    fn at(x: f32, y: f32, pressed: bool, active: bool, released: bool, delta: Vec2) -> Response {
        Response { mouse_pos: Vec2::new(x, y), pressed, active, released, drag_delta: delta, ..Default::default() }
    }

    #[test]
    fn a_click_never_moves_a_control() {
        let mut start = None;
        // The press frame carries the pointer's travel to the control (the
        // old code added this to the fader: the -0.2 dB drift).
        assert_eq!(drag_from(&mut start, &at(100.0, 200.0, true, true, false, Vec2::new(0.0, 3.0)), [0.8, 0.0]), None);
        // Jitter inside the dead zone while held: still nothing.
        assert_eq!(drag_from(&mut start, &at(101.0, 202.0, false, true, false, Vec2::new(1.0, 2.0)), [0.8, 0.0]), None);
        // Released: nothing, and the drag is over.
        assert_eq!(drag_from(&mut start, &at(101.0, 202.0, false, false, true, Vec2::ZERO), [0.8, 0.0]), None);
        assert!(start.is_none());
    }

    #[test]
    fn a_drag_is_measured_from_the_press() {
        let mut start = None;
        drag_from(&mut start, &at(100.0, 200.0, true, true, false, Vec2::new(5.0, 5.0)), [0.5, 0.0]);
        // Past the dead zone: offset from the press point, from the value
        // at the press, however the frames' deltas add up.
        let (v0, d) = drag_from(&mut start, &at(100.0, 150.0, false, true, false, Vec2::new(0.0, -50.0)), [0.5, 0.0]).unwrap();
        assert_eq!((v0, d), ([0.5, 0.0], Vec2::new(0.0, -50.0)));
        // Once moving, small offsets count (back towards the start).
        let (_, d) = drag_from(&mut start, &at(100.0, 199.0, false, true, false, Vec2::new(0.0, 49.0)), [0.9, 0.0]).unwrap();
        assert_eq!(d, Vec2::new(0.0, -1.0));
    }
}
