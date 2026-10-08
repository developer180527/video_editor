//! Glue between the editor UI and the shell, shared by the desktop and iPad
//! apps: the engine thread's waker, file dialogs, and the dock's windows.
//! It is the only crate that knows both `ve_ui` and `platform_winit`.

use std::path::PathBuf;

use libgui::{SurfaceId, Ui, Vec2};
use platform_winit::{DockHost, FileDialog, Gpu, ShellApp, ShellConfig, ShellCtx, SurfaceInfo};
use ve_engine::Engine;
use ve_model::MediaRef;
use ve_ui::{EditorUi, HostRequest};

struct App(EditorUi);

fn file_ref(p: &std::path::Path) -> MediaRef {
    MediaRef(format!("file:{}", p.display()))
}

impl ShellApp for App {
    fn prepare(&mut self, gpu: &Gpu, renderer: &mut libgui_wgpu::Renderer) {
        self.0.prepare_gpu(&gpu.device, &gpu.queue, renderer);
    }

    fn ui(&mut self, ui: &mut Ui, surface: SurfaceId, shell: &mut ShellCtx) {
        self.0.ui_for(ui, surface);
        for r in self.0.take_requests() {
            shell.file_dialog(match r {
                HostRequest::ImportMedia => FileDialog::OpenMedia,
                HostRequest::OpenProject => FileDialog::OpenProject,
                HostRequest::SaveProjectAs => FileDialog::SaveProject { default_name: format!("{}.veproj", self.0.project_name()) },
                HostRequest::ExportAs { default_name, extension } => FileDialog::SaveMedia { default_name, extension },
            });
        }
    }

    fn animating(&self) -> bool {
        self.0.animating()
    }

    fn dialog_result(&mut self, dialog: FileDialog, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let e = &self.0.engine;
        match dialog {
            FileDialog::OpenMedia => e.import(paths.iter().map(|p| p.to_string_lossy().into_owned()).collect()),
            FileDialog::OpenProject => e.open(file_ref(&paths[0])),
            FileDialog::SaveMedia { .. } => self.0.start_export(&paths[0]),
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
        self.0.dock_mut().set_pointer(screen, down);
    }
    fn set_pointer_down(&mut self, down: bool) {
        self.0.dock_mut().set_pointer_down(down);
    }
    fn set_surface_frame(&mut self, id: SurfaceId, origin: Vec2, scale: f32) {
        self.0.dock_mut().set_surface_frame(id, origin, scale);
    }
    fn update(&mut self) {
        self.0.dock_mut().update();
    }
    fn surfaces(&self) -> Vec<SurfaceInfo> {
        self.0
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
        self.0.dock_mut().close_surface(id);
    }
    fn is_dragging(&self) -> bool {
        self.0.dock().is_dragging()
    }
    fn cancel_drag(&mut self) {
        self.0.dock_mut().cancel_drag();
    }
    fn needs_frame(&self, id: SurfaceId) -> bool {
        self.0.dock().needs_frame(id)
    }
}

/// Run the editor: the engine on its own thread, the UI in the shell.
/// `touch` picks the tablet arrangement. Never returns on iPadOS.
pub fn run(engine: Engine, touch: bool) {
    let cfg = ShellConfig { theme: ve_ui::theme(), ..ShellConfig::default() };
    platform_winit::run(cfg, move |waker| {
        let mut ui = EditorUi::new(engine.spawn(waker), touch);
        // Hardware-decoded frames go to the compositor without a copy where
        // the platform can import them.
        ui.set_importer(gpu_import::importer());
        App(ui)
    });
}
