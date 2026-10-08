//! The shell that hosts the UI: windows, input, clipboard, file dialogs and
//! the wgpu surfaces libgui draws into. The same code runs on desktop and
//! iPadOS; the differences are `cfg`s in this crate.
//!
//! The app hands the shell a [`ShellApp`]. The shell knows nothing about
//! video editing, and the app knows nothing about winit.
//!
//! **Windows.** One OS window per dock surface: the main window, plus a real
//! window for every panel torn off it (desktop). An app with no dock, or a
//! platform without windows (iPadOS), gets the main window only. The host
//! logic follows the `libgui_cut` demo host: swapchains are reconfigured once
//! per drawn frame, and frames are gated on `needs_frame_for`.
//!
//! **Waking.** Other threads (the engine) wake the event loop through a
//! [`Waker`], so an idle window redraws when there is news and otherwise
//! sleeps.

#[cfg(target_os = "ios")]
mod ios_scene;
#[cfg(target_os = "ios")]
mod ios_picker;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use libgui::{Backend, Batch, Color, FrameInfo, InputEvent, PlatformOutput, PointerButton, SurfaceId, Theme, Ui, Vec2};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop, EventLoopProxy};
use winit::window::{Window, WindowId};

pub use libgui_winit::FILES as FILES_PAYLOAD;

/// libgui's bundled UI font (Inter, SIL OFL 1.1).
pub const DEFAULT_FONT: &[u8] = include_bytes!("../../../third_party/libgui/assets/Inter.ttf");

const IOS: bool = cfg!(target_os = "ios");

/// Wakes the shell from any thread.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// The GPU everything shares: the UI, the compositor, decoders' imports.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
}

/// A file dialog the app wants shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileDialog {
    /// Pick media files to import.
    OpenMedia,
    OpenProject,
    SaveProject { default_name: String },
    /// Where to write an export; `extension` without the dot.
    SaveMedia { default_name: String, extension: String },
}

/// One dock surface, as the shell needs to know it.
#[derive(Clone, Debug)]
pub struct SurfaceInfo {
    pub id: SurfaceId,
    pub visible: bool,
    pub floating: bool,
    pub window_size: Vec2,
    pub window_pos: Option<Vec2>,
    pub title: String,
}

/// What the shell needs from an app's dock to give torn-off panels their
/// own windows. Implemented by the app over its `libgui::DockState`.
pub trait DockHost {
    fn set_pointer(&mut self, screen: Vec2, down: bool);
    fn set_pointer_down(&mut self, down: bool);
    fn set_surface_frame(&mut self, id: SurfaceId, origin: Vec2, scale: f32);
    fn update(&mut self);
    fn surfaces(&self) -> Vec<SurfaceInfo>;
    fn close_surface(&mut self, id: SurfaceId);
    fn is_dragging(&self) -> bool;
    fn cancel_drag(&mut self);
    fn needs_frame(&self, id: SurfaceId) -> bool;
}

/// Requests the app makes of the shell during a frame.
#[derive(Default)]
pub struct ShellCtx {
    dialogs: Vec<FileDialog>,
}

impl ShellCtx {
    pub fn file_dialog(&mut self, d: FileDialog) {
        self.dialogs.push(d);
    }
}

pub trait ShellApp {
    /// Once, when the GPU is ready.
    fn gpu_ready(&mut self, _gpu: &Gpu, _renderer: &mut libgui_wgpu::Renderer) {}
    /// Every drawn frame of the main window, before its UI is built: render
    /// and register the textures the UI will show.
    fn prepare(&mut self, _gpu: &Gpu, _renderer: &mut libgui_wgpu::Renderer) {}
    /// Build one window's UI. `surface` is `SurfaceId::MAIN` for the main window.
    fn ui(&mut self, ui: &mut Ui, surface: SurfaceId, shell: &mut ShellCtx);
    /// True while something changes every frame on its own (playback).
    fn animating(&self) -> bool {
        false
    }
    /// The user answered a [`FileDialog`]; empty when cancelled.
    fn dialog_result(&mut self, _dialog: FileDialog, _paths: Vec<PathBuf>) {}
    /// The app's dock, for tear-off windows. `None`: one window only.
    fn dock(&mut self) -> Option<&mut dyn DockHost> {
        None
    }
}

pub struct ShellConfig {
    pub title: String,
    /// Initial window size in logical px (ignored on iPadOS: full screen).
    pub size: Vec2,
    pub theme: Theme,
    pub font: &'static [u8],
}

impl Default for ShellConfig {
    fn default() -> Self {
        ShellConfig { title: "Video Editor".into(), size: Vec2::new(1860.0, 1040.0), theme: Theme::dark(), font: DEFAULT_FONT }
    }
}

/// Messages to the event loop from other threads.
#[derive(Debug)]
pub enum UserEvent {
    Wake,
    /// The iPadOS document picker finished.
    #[allow(dead_code)]
    Picked(FileDialog, Vec<PathBuf>),
}

struct Gfx {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    gpu: Gpu,
}

struct Win {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: libgui_wgpu::Renderer,
    ui: Ui,
    platform: libgui_winit::PlatformState,
    files: libgui_winit::FileDrop,
    dock_id: SurfaceId,
    last: Instant,
    idle: f32,
    visible: bool,
    title: String,
    batches: Vec<Batch>,
    clear: Color,
    /// Woken by another thread: rebuild the UI on the next draw.
    rebuild: bool,
}

struct Shell<A: ShellApp> {
    cfg: ShellConfig,
    app: A,
    gfx: Option<Gfx>,
    wins: HashMap<WindowId, Win>,
    clipboard: Clipboard,
    #[cfg_attr(not(target_os = "ios"), allow(dead_code))]
    proxy: EventLoopProxy<UserEvent>,
    /// Inner minus outer position: what `set_outer_position` has to undo.
    decoration: Vec2,
    left_down: bool,
}

/// OS clipboard where there is one in reach (desktop).
struct Clipboard(#[cfg(not(target_os = "ios"))] Option<arboard::Clipboard>);

impl Clipboard {
    fn new() -> Self {
        #[cfg(not(target_os = "ios"))]
        return Clipboard(arboard::Clipboard::new().ok());
        #[cfg(target_os = "ios")]
        Clipboard()
    }
    fn get(&mut self) -> Option<String> {
        #[cfg(not(target_os = "ios"))]
        return self.0.as_mut().and_then(|c| c.get_text().ok());
        #[cfg(target_os = "ios")]
        None
    }
    fn set(&mut self, _text: String) {
        #[cfg(not(target_os = "ios"))]
        if let Some(c) = self.0.as_mut() {
            let _ = c.set_text(_text);
        }
    }
}

fn vec(p: PhysicalPosition<i32>) -> Vec2 {
    Vec2::new(p.x as f32, p.y as f32)
}

/// iOS reports the safe area as the inner size, but the layer covers the screen.
fn surface_size(w: &Window) -> winit::dpi::PhysicalSize<u32> {
    if IOS {
        w.outer_size()
    } else {
        w.inner_size()
    }
}

/// Top-left of the window's content in screen px: the dock hit-tests there.
fn content_origin(w: &Window) -> Vec2 {
    let p = if IOS { w.outer_position() } else { w.inner_position() };
    p.map(vec).unwrap_or(Vec2::ZERO)
}

impl<A: ShellApp> Shell<A> {
    fn create_window(&mut self, el: &ActiveEventLoop, dock_id: SurfaceId, title: &str, size: Vec2, inner_pos: Option<Vec2>) -> WindowId {
        let main = dock_id == SurfaceId::MAIN;
        let mut attrs = Window::default_attributes().with_title(title).with_active(main);
        if !IOS {
            attrs = attrs.with_inner_size(LogicalSize::new(size.x, size.y));
        }
        if let Some(p) = inner_pos {
            let outer = p - self.decoration;
            attrs = attrs.with_position(PhysicalPosition::new(outer.x as i32, outer.y as i32));
        }
        let window = Arc::new(el.create_window(attrs).expect("window"));
        #[cfg(target_os = "ios")]
        ios_scene::window_created(&window);

        if self.gfx.is_none() {
            let instance =
                wgpu::Instance::new(wgpu::InstanceDescriptor::new_with_display_handle(Box::new(el.owned_display_handle())));
            let probe = instance.create_surface(window.clone()).expect("surface");
            let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&probe),
                ..Default::default()
            }))
            .expect("no GPU adapter");
            // 16-bit normalized textures carry 10-bit video to the compositor.
            let features = adapter.features() & wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
            let desc = wgpu::DeviceDescriptor { required_features: features, required_limits: adapter.limits(), ..Default::default() };
            let (device, queue) = pollster::block_on(adapter.request_device(&desc)).expect("device");
            drop(probe);
            self.gfx = Some(Gfx { instance, adapter, gpu: Gpu { device, queue } });
        }
        let g = self.gfx.as_ref().unwrap();
        let surface = g.instance.create_surface(window.clone()).expect("surface");
        let px = surface_size(&window);
        let mut config = surface.get_default_config(&g.adapter, px.width.max(1), px.height.max(1)).expect("surface config");
        let caps = surface.get_capabilities(&g.adapter);
        config.format = caps.formats.iter().copied().find(|f| !f.is_srgb()).unwrap_or(caps.formats[0]);
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&g.gpu.device, &config);
        let mut renderer = libgui_wgpu::Renderer::new(&g.gpu.device, &g.gpu.queue, config.format);
        if main {
            self.app.gpu_ready(&g.gpu, &mut renderer);
            if let (Ok(o), Ok(i)) = (window.outer_position(), window.inner_position()) {
                self.decoration = vec(i) - vec(o);
            }
        }

        // Every window has its own `Ui`; secondary ones share the main one's fonts.
        let mut ui = match self.wins.values().find(|w| w.dock_id == SurfaceId::MAIN) {
            Some(m) => Ui::sharing_fonts(self.cfg.theme.clone(), &m.ui),
            None => Ui::new(self.cfg.theme.clone(), self.cfg.font).expect("font"),
        };
        ui.theme = self.cfg.theme.clone();
        libgui_keymap::Keymap::<u8>::for_current_platform().install(&mut ui);
        ui.reserve(8_000);
        let clear = ui.theme.palette.bg_app;
        let id = window.id();
        self.wins.insert(
            id,
            Win {
                window,
                surface,
                config,
                renderer,
                ui,
                platform: Default::default(),
                files: Default::default(),
                dock_id,
                last: Instant::now(),
                idle: 0.0,
                visible: true,
                title: title.to_string(),
                batches: Vec::new(),
                clear,
                rebuild: true,
            },
        );
        id
    }

    fn report_frame(&mut self, wid: WindowId) {
        let Some(w) = self.wins.get(&wid) else { return };
        let (id, origin, scale) = (w.dock_id, content_origin(&w.window), w.window.scale_factor() as f32);
        if let Some(d) = self.app.dock() {
            d.set_surface_frame(id, origin, scale);
        }
    }

    /// Make the OS windows match the dock: create, destroy, move, show, hide.
    fn sync_windows(&mut self, el: &ActiveEventLoop) {
        if IOS {
            return; // one window; panels float inside it
        }
        let Some(surfaces) = self.app.dock().map(|d| d.surfaces()) else { return };
        self.wins.retain(|_, w| surfaces.iter().any(|s| s.id == w.dock_id));
        // A torn-off tab gets a window once it is actually visible (dropped
        // on the desktop), not while it hovers over drop targets.
        let missing: Vec<SurfaceInfo> = surfaces
            .iter()
            .filter(|s| s.visible && !self.wins.values().any(|w| w.dock_id == s.id))
            .cloned()
            .collect();
        for s in missing {
            let wid = self.create_window(el, s.id, &s.title, s.window_size, s.window_pos);
            self.report_frame(wid);
            self.draw(wid);
        }
        for w in self.wins.values_mut() {
            let Some(s) = surfaces.iter().find(|s| s.id == w.dock_id) else { continue };
            if let Some(p) = s.window_pos {
                let outer = p - self.decoration;
                w.window.set_outer_position(PhysicalPosition::new(outer.x.round() as i32, outer.y.round() as i32));
            }
            if s.visible != w.visible {
                w.visible = s.visible;
                w.window.set_visible(s.visible);
            }
            if w.dock_id != SurfaceId::MAIN && s.title != w.title {
                w.title = s.title.clone();
                w.window.set_title(&s.title);
            }
        }
        let ids: Vec<WindowId> = self.wins.keys().copied().collect();
        for wid in ids {
            self.report_frame(wid);
        }
    }

    fn draw(&mut self, wid: WindowId) {
        let Some(mut w) = self.wins.remove(&wid) else { return };
        let Some(g) = self.gfx.as_ref() else {
            self.wins.insert(wid, w);
            return;
        };
        let px = surface_size(&w.window);
        if px.width.max(1) != w.config.width || px.height.max(1) != w.config.height {
            w.config.width = px.width.max(1);
            w.config.height = px.height.max(1);
            w.surface.configure(&g.gpu.device, &w.config);
        }
        let now = Instant::now();
        w.idle += (now - w.last).as_secs_f32();
        w.last = now;
        let scale = w.window.scale_factor() as f32;
        let info = FrameInfo {
            screen_size: Vec2::new(w.config.width as f32 / scale, w.config.height as f32 / scale),
            scale,
            dt: w.idle,
        };

        let main = w.dock_id == SurfaceId::MAIN;
        let dock_wants = self.app.dock().is_some_and(|d| d.needs_frame(w.dock_id));
        let mut platform = PlatformOutput::default();
        let mut ctx = ShellCtx::default();
        if w.rebuild || self.app.animating() || dock_wants || w.ui.needs_frame_for(&info, w.idle) {
            w.idle = 0.0;
            w.rebuild = false;
            if main {
                self.app.prepare(&g.gpu, &mut w.renderer);
            }
            w.ui.begin_frame(info);
            self.app.ui(&mut w.ui, w.dock_id, &mut ctx);
            let out = w.ui.end_frame();
            platform = out.platform.clone();
            w.clear = out.clear_color;
            w.renderer.prepare(&out);
            w.batches.clear();
            w.batches.extend_from_slice(&out.draw.batches);
        }

        let frame = match w.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(f) | wgpu::CurrentSurfaceTexture::Suboptimal(f) => Some(f),
            _ => {
                w.surface.configure(&g.gpu.device, &w.config);
                None
            }
        };
        if let Some(frame) = frame {
            let view = frame.texture.create_view(&Default::default());
            let mut enc = g.gpu.device.create_command_encoder(&Default::default());
            {
                let c = w.clear;
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("ui"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: c.r as f64, g: c.g as f64, b: c.b as f64, a: 1.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                w.renderer.render_batches(&mut pass, &w.batches);
            }
            g.gpu.queue.submit([enc.finish()]);
            w.window.pre_present_notify();
            g.gpu.queue.present(frame);
        }
        w.platform.apply(&w.window, &platform);
        if let Some(text) = platform.copied_text {
            self.clipboard.set(text);
        }
        if platform.paste_requested {
            if let Some(text) = self.clipboard.get() {
                w.ui.push(InputEvent::Paste(text));
            }
        }
        self.wins.insert(wid, w);
        for d in ctx.dialogs {
            self.show_dialog(d);
        }
    }

    /// Desktop: a native dialog, answered at once. iPadOS: the document
    /// picker, answered later through `UserEvent::Picked`.
    fn show_dialog(&mut self, d: FileDialog) {
        #[cfg(not(target_os = "ios"))]
        {
            let media = ["mov", "mp4", "m4v", "mkv", "mxf", "avi", "webm", "mts", "wav", "aif", "aiff", "mp3", "m4a", "flac", "png", "jpg", "jpeg"];
            let paths: Vec<PathBuf> = match &d {
                FileDialog::OpenMedia => {
                    rfd::FileDialog::new().set_title("Import Media").add_filter("Media", &media).pick_files().unwrap_or_default()
                }
                FileDialog::OpenProject => rfd::FileDialog::new()
                    .set_title("Open Project")
                    .add_filter("Project", &["veproj"])
                    .pick_file()
                    .into_iter()
                    .collect(),
                FileDialog::SaveMedia { default_name, extension } => rfd::FileDialog::new()
                    .set_title("Export")
                    .add_filter("Movie", &[extension.as_str()])
                    .set_file_name(default_name)
                    .save_file()
                    .into_iter()
                    .collect(),
                FileDialog::SaveProject { default_name } => rfd::FileDialog::new()
                    .set_title("Save Project")
                    .add_filter("Project", &["veproj"])
                    .set_file_name(default_name)
                    .save_file()
                    .into_iter()
                    .collect(),
            };
            self.app.dialog_result(d, paths);
        }
        #[cfg(target_os = "ios")]
        {
            let window = self.wins.values().find(|w| w.dock_id == SurfaceId::MAIN).map(|w| w.window.clone());
            if let Some(window) = window {
                ios_picker::present(&window, d, self.proxy.clone());
            }
        }
    }
}

impl<A: ShellApp> ApplicationHandler<UserEvent> for Shell<A> {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.wins.is_empty() {
            let title = self.cfg.title.clone();
            let size = self.cfg.size;
            self.create_window(el, SurfaceId::MAIN, &title, size, None);
        }
    }

    fn user_event(&mut self, _el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => {}
            UserEvent::Picked(d, paths) => self.app.dialog_result(d, paths),
        }
        // News from another thread: the UI must be rebuilt, not just
        // re-presented. (libgui's own repaint request only counts mid-frame.)
        for w in self.wins.values_mut() {
            w.rebuild = true;
            w.window.request_redraw();
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, wid: WindowId, event: WindowEvent) {
        let Some(w) = self.wins.get_mut(&wid) else { return };
        if !w.files.push_window_event(&mut w.ui, &event) {
            libgui_winit::push_window_event(&mut w.ui, &event, w.window.scale_factor());
        }
        let dock_id = w.dock_id;
        let origin = w.window.inner_position().ok().map(vec);
        match event {
            WindowEvent::CloseRequested => {
                if dock_id == SurfaceId::MAIN {
                    el.exit();
                } else if let Some(d) = self.app.dock() {
                    d.close_surface(dock_id);
                }
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => w.window.request_redraw(),
            WindowEvent::RedrawRequested => self.draw(wid),
            WindowEvent::CursorMoved { position, .. } => {
                // The dock hit-tests in screen coordinates: during a tab drag
                // the source window keeps receiving moves outside its bounds.
                if let Some(o) = origin {
                    let screen = o + Vec2::new(position.x as f32, position.y as f32);
                    let down = self.left_down;
                    if let Some(d) = self.app.dock() {
                        d.set_pointer(screen, down);
                    }
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let down = state == ElementState::Pressed;
                if let Some(d) = self.app.dock() {
                    if down && d.is_dragging() {
                        d.cancel_drag(); // a press while "dragging" means a missed release
                    }
                    d.set_pointer_down(down);
                }
                self.left_down = down;
                if !down {
                    // A drag may end over another window: release everywhere.
                    let up = InputEvent::PointerButton { button: PointerButton::Primary, pressed: false };
                    for (id, other) in self.wins.iter_mut() {
                        if *id != wid {
                            other.ui.push(up.clone());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _el: &ActiveEventLoop, _id: winit::event::DeviceId, event: winit::event::DeviceEvent) {
        for w in self.wins.values_mut() {
            libgui_winit::push_device_event(&mut w.ui, &event);
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.gfx.is_none() {
            return;
        }
        if let Some(d) = self.app.dock() {
            d.update();
        }
        self.sync_windows(el);
        for w in self.wins.values() {
            if w.visible {
                w.window.request_redraw();
            }
        }
    }
}

/// Run until the main window closes. `make_app` receives the [`Waker`] other
/// threads use to wake the UI. Never returns on iPadOS.
pub fn run<A: ShellApp + 'static>(cfg: ShellConfig, make_app: impl FnOnce(Waker) -> A) {
    #[cfg(target_os = "ios")]
    ios_scene::register();
    let el = EventLoop::<UserEvent>::with_user_event().build().expect("event loop");
    let proxy = el.create_proxy();
    let p = proxy.clone();
    let waker: Waker = Arc::new(move || {
        let _ = p.send_event(UserEvent::Wake);
    });
    let mut shell = Shell {
        app: make_app(waker),
        cfg,
        gfx: None,
        wins: HashMap::new(),
        clipboard: Clipboard::new(),
        proxy,
        decoration: Vec2::ZERO,
        left_down: false,
    };
    el.run_app(&mut shell).expect("event loop");
}
