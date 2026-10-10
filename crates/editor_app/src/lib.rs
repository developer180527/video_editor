//! Glue between the editor UI and the shell, shared by the desktop and iPad
//! apps: the engine thread's waker, file dialogs, and the dock's windows.
//! It is the only crate that knows both `ve_ui` and `platform_winit`.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ve_ports::{Location, Storage};

use libgui::{SurfaceId, Ui, Vec2};
use libgui_keymap::Platform;
use platform_winit::{Chrome, DockHost, FileDialog, Gpu, MenuEntry, NativeMenu, ShellApp, ShellConfig, ShellCtx, SurfaceInfo, WindowGeometry, WindowRequest};
use ve_engine::Engine;
use ve_model::MediaRef;
use ve_ui::{Action, EditorUi, Entry, HostRequest, WindowAction, WindowControls, WindowFrame};

struct App {
    ui: EditorUi,
    /// What the open media picker is for when it is not an import (the
    /// dialog itself does not say).
    pick: Option<HostRequest>,
    /// The actions behind the system menu bar's item ids.
    menu_actions: Vec<Action>,
    store: Store,
    /// The main window's frame, as last drawn: saved with the layout.
    main_window: WindowGeometry,
}

/// Where the editor keeps its settings and layout: the app-data folder.
struct Store(Arc<dyn Storage>);

const SETTINGS_FILE: &str = "settings.toml";
const LAYOUT_FILE: &str = "layout.toml";
const WINDOW_FILE: &str = "window.toml";

/// The main window's saved frame.
#[derive(Serialize, Deserialize)]
struct SavedWindow {
    x: Option<i32>,
    y: Option<i32>,
    width: f32,
    height: f32,
    maximized: bool,
}

impl Store {
    fn read(&self, name: &str) -> Option<String> {
        let r = self.0.location(Location::AppData, name).ok()?;
        let mut text = String::new();
        self.0.open_read(&r).ok()?.read_to_string(&mut text).ok()?;
        Some(text)
    }

    fn write(&self, name: &str, text: &str) -> Result<(), String> {
        let r = self.0.location(Location::AppData, name).map_err(|e| e.to_string())?;
        self.0.write_atomic(&r, text.as_bytes()).map_err(|e| format!("Could not save {name}: {e}"))
    }
}

/// The editor's menus as the system menu bar takes them: item ids index
/// `actions`.
fn native_menus(menus: Vec<ve_ui::Menu>, actions: &mut Vec<Action>) -> Vec<NativeMenu> {
    actions.clear();
    let platform = Platform::current();
    menus
        .into_iter()
        .map(|m| NativeMenu {
            title: m.title,
            entries: m
                .entries
                .into_iter()
                .map(|e| match e {
                    Entry::Separator => MenuEntry::Separator,
                    Entry::Item(it) => {
                        actions.push(it.action);
                        MenuEntry::Item {
                            id: actions.len() as u32 - 1,
                            label: it.label,
                            shortcut: it.shortcut.map(|c| c.resolve(platform)),
                            enabled: it.enabled,
                            checked: it.checked == Some(true),
                        }
                    }
                })
                .collect(),
        })
        .collect()
}

fn file_ref(p: &std::path::Path) -> MediaRef {
    MediaRef(format!("file:{}", p.display()))
}

impl App {
    /// The dock layout and the main window's frame, for the next start.
    fn save_layout(&mut self) {
        let g = self.main_window;
        let window = SavedWindow { x: g.position.map(|p| p.0), y: g.position.map(|p| p.1), width: g.size.x, height: g.size.y, maximized: g.maximized };
        let result = match self.ui.layout_toml() {
            Some(layout) => self
                .store
                .write(LAYOUT_FILE, &layout)
                .and_then(|_| self.store.write(WINDOW_FILE, &toml::to_string(&window).unwrap_or_default())),
            None => Err("Could not save the layout.".into()),
        };
        if let Err(e) = result {
            self.ui.report_error(e);
        }
    }
}

impl ShellApp for App {
    fn prepare(&mut self, gpu: &Gpu, renderer: &mut libgui_wgpu::Renderer, surface: SurfaceId) {
        if surface == SurfaceId::MAIN {
            self.ui.prepare_gpu(&gpu.device, &gpu.queue, renderer);
        } else {
            // A torn-off panel shows the same monitor and thumbnails.
            self.ui.share_textures(surface, renderer);
        }
    }

    fn ui(&mut self, ui: &mut Ui, surface: SurfaceId, shell: &mut ShellCtx) {
        let controls = match shell.chrome() {
            Chrome::Os => WindowControls::Os,
            Chrome::Leading { inset } => WindowControls::Leading { inset },
            Chrome::Drawn { maximized } => WindowControls::Drawn { maximized },
        };
        self.ui.ui_framed(ui, surface, WindowFrame { controls, system_menu: shell.system_menu() });
        shell.set_title_strip(self.ui.title_strip(surface));
        if surface == SurfaceId::MAIN {
            self.main_window = shell.geometry();
        }
        for (s, a) in self.ui.take_window_actions() {
            if s == surface {
                shell.window(match a {
                    WindowAction::Minimize => WindowRequest::Minimize,
                    WindowAction::ToggleMaximize => WindowRequest::ToggleMaximize,
                    WindowAction::Close => WindowRequest::Close,
                });
            }
        }
        if surface == SurfaceId::MAIN && shell.system_menu() {
            let menus = self.ui.menus();
            shell.set_menu(native_menus(menus, &mut self.menu_actions));
        }
        for r in self.ui.take_requests() {
            match r {
                HostRequest::SaveSettings => {
                    if let Err(e) = self.store.write(SETTINGS_FILE, &self.ui.settings_toml()) {
                        self.ui.report_error(e);
                    }
                    continue;
                }
                HostRequest::SaveLayout => {
                    self.save_layout();
                    continue;
                }
                _ => {}
            }
            shell.file_dialog(match r {
                HostRequest::SaveSettings | HostRequest::SaveLayout => unreachable!(),
                HostRequest::ImportMedia => {
                    self.pick = None;
                    FileDialog::OpenMedia
                }
                HostRequest::AttachProxy(_) | HostRequest::RelinkMedia(_) => {
                    self.pick = Some(r);
                    FileDialog::OpenMedia
                }
                HostRequest::OpenProject => FileDialog::OpenProject,
                HostRequest::SaveProjectAs => FileDialog::SaveProject { default_name: format!("{}.veproj", self.ui.project_name()) },
                HostRequest::ExportAs { default_name, extension } => FileDialog::SaveMedia { default_name, extension },
            });
        }
    }

    fn menu_action(&mut self, id: u32) {
        if let Some(a) = self.menu_actions.get(id as usize).cloned() {
            self.ui.perform(&a);
        }
    }

    fn animating(&self) -> bool {
        self.ui.animating()
    }

    fn dialog_result(&mut self, dialog: FileDialog, paths: Vec<PathBuf>) {
        let purpose = if matches!(dialog, FileDialog::OpenMedia) { self.pick.take() } else { None };
        if paths.is_empty() {
            return;
        }
        match purpose {
            Some(HostRequest::AttachProxy(asset)) => return self.ui.attach_proxy(asset, &paths[0]),
            Some(HostRequest::RelinkMedia(asset)) => return self.ui.relink(asset, &paths[0]),
            _ => {}
        }
        let e = &self.ui.engine;
        match dialog {
            FileDialog::OpenMedia => e.import(paths.iter().map(|p| p.to_string_lossy().into_owned()).collect()),
            FileDialog::OpenProject => e.open(file_ref(&paths[0])),
            FileDialog::SaveMedia { .. } => self.ui.start_export(&paths[0]),
            FileDialog::SaveProject { .. } => {
                let mut p = paths[0].clone();
                if p.extension().is_none() {
                    p.set_extension("veproj");
                }
                e.save_as(file_ref(&p));
            }
        }
    }

    fn dock(&mut self) -> Option<&mut dyn DockHost> {
        Some(self)
    }
}

impl DockHost for App {
    fn set_pointer(&mut self, screen: Vec2, down: bool) {
        self.ui.dock_mut().set_pointer(screen, down);
    }
    fn set_pointer_down(&mut self, down: bool) {
        self.ui.dock_mut().set_pointer_down(down);
    }
    fn set_surface_frame(&mut self, id: SurfaceId, origin: Vec2, scale: f32) {
        self.ui.dock_mut().set_surface_frame(id, origin, scale);
    }
    fn update(&mut self) {
        self.ui.dock_mut().update();
    }
    fn surfaces(&self) -> Vec<SurfaceInfo> {
        self.ui
            .dock()
            .surfaces()
            .iter()
            .map(|s| SurfaceInfo {
                id: s.id,
                visible: s.visible,
                floating: s.floating,
                window_size: s.window_size,
                window_pos: s.window_pos,
                title: s.first_tab().map(|t| t.name().to_string()).unwrap_or_else(|| "Video Editor".into()),
            })
            .collect()
    }
    fn close_surface(&mut self, id: SurfaceId) {
        self.ui.dock_mut().close_surface(id);
    }
    fn is_dragging(&self) -> bool {
        self.ui.dock().is_dragging()
    }
    fn cancel_drag(&mut self) {
        self.ui.dock_mut().cancel_drag();
    }
    fn needs_frame(&self, id: SurfaceId) -> bool {
        self.ui.dock().needs_frame(id)
    }
}

/// Run the editor: the engine on its own thread, the UI in the shell.
/// `touch` picks the tablet arrangement. Never returns on iPadOS.
pub fn run(engine: Engine, touch: bool) {
    let store = Store(engine.platform().storage.clone());
    let settings = store.read(SETTINGS_FILE).and_then(|t| ve_ui::Settings::from_toml(&t)).unwrap_or_default();
    let layout = settings.restore_layout.then(|| store.read(LAYOUT_FILE)).flatten();
    let window = layout.as_ref().and_then(|_| store.read(WINDOW_FILE)).and_then(|t| toml::from_str::<SavedWindow>(&t).ok());
    let mut cfg = ShellConfig { theme: ve_ui::theme(), ..ShellConfig::default() };
    if let Some(w) = &window {
        cfg.size = Vec2::new(w.width.max(640.0), w.height.max(400.0));
        cfg.position = w.x.zip(w.y);
        cfg.maximized = w.maximized;
    }
    platform_winit::run(cfg, move |waker| {
        let mut ui = EditorUi::new(engine.spawn(waker), touch);
        // Hardware-decoded frames go to the compositor without a copy where
        // the platform can import them.
        ui.set_importer(gpu_import::importer());
        ui.load_settings(settings);
        if let Some(text) = layout {
            ui.restore_layout(&text);
        }
        App { ui, pick: None, menu_actions: Vec::new(), store, main_window: WindowGeometry::default() }
    });
}
