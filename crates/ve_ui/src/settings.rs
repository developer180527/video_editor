//! The app's settings: what they are, the panel that edits them, and how
//! they reach the editor. The host stores them ([`EditorUi::settings_toml`]
//! when [`crate::HostRequest::SaveSettings`] comes); the UI never touches
//! files.

use libgui::*;
use serde::{Deserialize, Serialize};

use crate::EditorUi;

/// Everything on the Settings panel. Missing keys take their defaults, so
/// a file from an older version still loads.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Program monitor resolution at startup: 0 full, 1 half, 2 quarter.
    pub preview_quality: usize,
    /// Play proxies at startup, where assets have them.
    pub use_proxies: bool,
    /// Snapping at startup.
    pub snap: bool,
    /// Linked selection at startup.
    pub linked_selection: bool,
    /// Length of a new transition, seconds.
    pub transition_seconds: f32,
    /// Length of a new title, matte or bars clip, seconds.
    pub still_seconds: f32,
    /// Put the saved window layout back at startup.
    pub restore_layout: bool,
    /// Light, dark, or as the system is set.
    #[serde(default)]
    pub theme: ThemeMode,
}

/// The Theme setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeMode {
    /// The look to show, given whether the system is in dark mode (unknown
    /// counts as dark: the editor's home look).
    pub fn appearance(self, system_dark: Option<bool>) -> crate::theme::Appearance {
        use crate::theme::Appearance;
        match self {
            ThemeMode::Light => Appearance::Light,
            ThemeMode::Dark => Appearance::Dark,
            ThemeMode::System if system_dark == Some(false) => Appearance::Light,
            ThemeMode::System => Appearance::Dark,
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            preview_quality: 1,
            use_proxies: false,
            snap: true,
            linked_selection: true,
            transition_seconds: 1.0,
            still_seconds: 5.0,
            restore_layout: true,
            theme: ThemeMode::System,
        }
    }
}

impl Settings {
    pub fn from_toml(text: &str) -> Option<Settings> {
        toml::from_str(text).ok()
    }
}

impl EditorUi {
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// Take `s` as the settings, and start the session as they say: the
    /// startup defaults apply now (call before the first frame).
    pub fn load_settings(&mut self, s: Settings) {
        self.view.quality = s.preview_quality.min(2);
        self.view.proxies = s.use_proxies;
        self.view.snap = s.snap;
        self.view.linked = s.linked_selection;
        self.settings = s;
    }

    /// The settings as the host stores them.
    pub fn settings_toml(&self) -> String {
        toml::to_string_pretty(&self.settings).unwrap_or_default()
    }
}

/// One labelled row: the label in a fixed column, the control after it.
fn row<R>(ui: &mut Ui, label: &str, control: impl FnOnce(&mut Ui) -> R) -> R {
    let t = ui.theme.clone();
    ui.container(Layout::row().width(Size::Grow(1.0)).height(Size::Fixed(26.0)).gap(10.0).align(Align::Start, Align::Center), Frame::none(), |ui| {
        ui.container(Layout::row().width(Size::Fixed(190.0)).height(Size::Grow(1.0)).align(Align::Start, Align::Center), Frame::none(), |ui| {
            ui.text_with(label, t.metrics.font_size, t.palette.text_muted)
        });
        ui.container(Layout::row().width(Size::Fixed(170.0)).height(Size::Fixed(20.0)).align(Align::Start, Align::Center), Frame::none(), control)
    })
}

/// The Settings panel.
pub(crate) fn panel(ui: &mut Ui, app: &mut EditorUi) {
    let t = ui.theme.clone();
    let mut s = app.settings.clone();
    let mut commit = false;

    ui.text_with("Appearance", t.metrics.font_size_heading, t.palette.text);
    let mut mode = s.theme as usize;
    row(ui, "Theme", |ui| ui.combo_keyed("set-theme", &mut mode, &["System", "Light", "Dark"]));
    let picked = [ThemeMode::System, ThemeMode::Light, ThemeMode::Dark][mode.min(2)];
    commit |= picked != s.theme;
    s.theme = picked;
    ui.space(10.0);

    ui.text_with("Playback", t.metrics.font_size_heading, t.palette.text);
    ui.text_with("Used when the editor starts.", t.metrics.font_size_small, t.palette.text_faint);
    row(ui, "Preview quality", |ui| ui.combo_keyed("set-quality", &mut s.preview_quality, &["Full", "1/2", "1/4"]));
    commit |= s.preview_quality != app.settings.preview_quality;
    commit |= row(ui, "Play proxies", |ui| ui.checkbox_keyed("set-proxies", "", &mut s.use_proxies).clicked);
    ui.space(10.0);

    ui.text_with("Timeline", t.metrics.font_size_heading, t.palette.text);
    commit |= row(ui, "Snap at startup", |ui| ui.checkbox_keyed("set-snap", "", &mut s.snap).clicked);
    commit |= row(ui, "Linked selection at startup", |ui| ui.checkbox_keyed("set-linked", "", &mut s.linked_selection).clicked);
    commit |= row(ui, "Transition length (s)", |ui| ui.drag_value_range_keyed("set-trans", "", &mut s.transition_seconds, 0.05, 0.1..=10.0).released);
    commit |= row(ui, "Title and matte length (s)", |ui| ui.drag_value_range_keyed("set-still", "", &mut s.still_seconds, 0.1, 0.5..=60.0).released);
    ui.space(10.0);

    ui.text_with("Workspace", t.metrics.font_size_heading, t.palette.text);
    commit |= row(ui, "Restore saved layout", |ui| ui.checkbox_keyed("set-layout", "", &mut s.restore_layout).clicked);
    ui.text_with("Save the current layout with View › Save Window Layout.", t.metrics.font_size_small, t.palette.text_faint);
    ui.space(14.0);
    if ui.button_keyed("set-defaults", "Restore Defaults").clicked {
        s = Settings::default();
        commit = true;
    }

    // Drags show their value as they go; a release (or a click) is saved.
    if s != app.settings {
        app.settings = s;
    }
    if commit {
        app.requests.push(crate::HostRequest::SaveSettings);
    }
}
