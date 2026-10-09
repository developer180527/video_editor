//! The menus, as data: the same model is drawn in the window (where menus
//! live in the window) or handed to the host for the system menu bar
//! (macOS). Choosing an item, either way, is [`EditorUi::perform`].

use libgui::*;
use libgui_keymap::{Chord, Platform};
use ve_engine::{edit, intrinsic};
use ve_model::*;

use crate::dock::{self, Tab};
use crate::{EditorUi, HostRequest};

/// Something a menu item does.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Import,
    OpenProject,
    Save,
    SaveAs,
    AttachProxy,
    RemoveProxy,
    LinkMedia,
    Export,
    Undo,
    Redo,
    Speed,
    FrameHold,
    Nest,
    Channels,
    AddEdit,
    VideoTransition,
    AudioTransition,
    ToggleProxies,
    OpenSequence(SequenceId),
    MarkIn,
    MarkOut,
    ClearInOut,
    AddMarker,
    NextMarker,
    PrevMarker,
    EditMarker,
    NewTitle,
    NewColorMatte,
    NewBars,
    /// Show a panel that is closed, or close one that is open.
    TogglePanel(Tab),
    ResetWorkspace,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MenuItem {
    pub label: String,
    pub action: Action,
    /// The chord that does the same thing (the editor's own shortcut).
    pub shortcut: Option<Chord>,
    pub enabled: bool,
    /// `Some` for a check item.
    pub checked: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    Item(MenuItem),
    Separator,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Menu {
    pub title: String,
    pub entries: Vec<Entry>,
}

/// Builds one menu's entries.
struct Builder(Vec<Entry>);

impl Builder {
    fn item(&mut self, label: impl Into<String>, action: Action, shortcut: Option<Chord>, enabled: bool) -> &mut Self {
        self.0.push(Entry::Item(MenuItem { label: label.into(), action, shortcut, enabled, checked: None }));
        self
    }
    fn on(&mut self, label: impl Into<String>, action: Action, shortcut: Option<Chord>) -> &mut Self {
        self.item(label, action, shortcut, true)
    }
    fn check(&mut self, label: impl Into<String>, action: Action, checked: bool) -> &mut Self {
        self.0.push(Entry::Item(MenuItem { label: label.into(), action, shortcut: None, enabled: true, checked: Some(checked) }));
        self
    }
    fn sep(&mut self) -> &mut Self {
        self.0.push(Entry::Separator);
        self
    }
    fn menu(&mut self, title: &str) -> Menu {
        Menu { title: title.into(), entries: std::mem::take(&mut self.0) }
    }
}

const fn key(k: Key) -> Option<Chord> {
    Some(Chord::key(k))
}
const fn primary(k: Key) -> Option<Chord> {
    Some(Chord::primary(k))
}

impl EditorUi {
    /// The menu bar, for the current state (what is enabled, what is checked).
    pub fn menus(&self) -> Vec<Menu> {
        let snap = self.snap();
        let has_clip = self.subject_clip().is_some();
        let sel = !self.view.selection.is_empty();
        let asset = self.view.selected_asset.filter(|a| snap.assets.contains_key(a));
        let has_proxy = asset.and_then(|a| snap.assets.get(&a)).is_some_and(|a| a.variants.iter().any(|v| v.kind == VariantKind::Proxy));
        let mut b = Builder(Vec::new());
        let mut out = Vec::new();

        b.on("Import…", Action::Import, primary(Key::I))
            .on("Open Project…", Action::OpenProject, primary(Key::O))
            .sep()
            .on("Save", Action::Save, primary(Key::S))
            .on("Save As…", Action::SaveAs, Some(Chord::primary(Key::S).shift()))
            .sep()
            // The selected bin item's media (also on its context menu).
            .item("Attach Proxy…", Action::AttachProxy, None, asset.is_some())
            .item("Remove Proxy", Action::RemoveProxy, None, has_proxy)
            .item("Link Media…", Action::LinkMedia, None, asset.is_some())
            .sep()
            .on("Export Media…", Action::Export, primary(Key::M));
        out.push(b.menu("File"));

        let undo = self.st.undo_label.clone().map_or("Undo".into(), |l| format!("Undo {l}"));
        let redo = self.st.redo_label.clone().map_or("Redo".into(), |l| format!("Redo {l}"));
        b.item(undo, Action::Undo, primary(Key::Z), self.st.undo_label.is_some())
            .item(redo, Action::Redo, Some(Chord::primary(Key::Z).shift()), self.st.redo_label.is_some());
        out.push(b.menu("Edit"));

        b.item("Speed/Duration…", Action::Speed, primary(Key::R), has_clip)
            .item("Add Frame Hold", Action::FrameHold, None, has_clip)
            .sep()
            .item("Nest…", Action::Nest, None, sel)
            .item("Audio Channels…", Action::Channels, None, has_clip);
        out.push(b.menu("Clip"));

        b.on("Add Edit", Action::AddEdit, primary(Key::K))
            .sep()
            .on("Apply Video Transition", Action::VideoTransition, primary(Key::D))
            .on("Apply Audio Transition", Action::AudioTransition, Some(Chord::primary(Key::D).shift()))
            .sep()
            .check("Use Proxies", Action::ToggleProxies, self.view.proxies);
        let seqs: Vec<(SequenceId, String)> = snap.sequences.values().map(|s| (s.id, s.name.clone())).collect();
        if seqs.len() > 1 {
            b.sep();
            for (id, name) in seqs {
                b.check(name, Action::OpenSequence(id), snap.active_sequence == Some(id));
            }
        }
        out.push(b.menu("Sequence"));

        b.on("Mark In", Action::MarkIn, key(Key::I))
            .on("Mark Out", Action::MarkOut, key(Key::O))
            .on("Clear In and Out", Action::ClearInOut, Some(Chord::key(Key::X).alt()))
            .sep()
            .on("Add Marker", Action::AddMarker, key(Key::M))
            .on("Go to Next Marker", Action::NextMarker, Some(Chord::key(Key::M).shift()))
            .on("Go to Previous Marker", Action::PrevMarker, Some(Chord::key(Key::M).shift().alt()))
            .item("Edit Marker…", Action::EditMarker, None, self.view.selected_marker.is_some());
        out.push(b.menu("Markers"));

        b.on("New Title", Action::NewTitle, None).on("New Color Matte", Action::NewColorMatte, None).on("New Bars", Action::NewBars, None);
        out.push(b.menu("Graphics"));

        let open = dock::open_tabs(&self.dock);
        for tab in Tab::ALL {
            b.check(tab.name(), Action::TogglePanel(tab), open.contains(&tab));
        }
        b.sep().on("Reset Workspace", Action::ResetWorkspace, None);
        out.push(b.menu("View"));
        out
    }

    /// Do what a menu item says.
    pub fn perform(&mut self, action: &Action) {
        match action {
            Action::Import => self.requests.push(HostRequest::ImportMedia),
            Action::OpenProject => self.requests.push(HostRequest::OpenProject),
            Action::Save => match self.st.file.clone() {
                Some(f) => self.engine.save_as(f),
                None => self.requests.push(HostRequest::SaveProjectAs),
            },
            Action::SaveAs => self.requests.push(HostRequest::SaveProjectAs),
            Action::AttachProxy => self.requests.extend(self.view.selected_asset.map(HostRequest::AttachProxy)),
            Action::RemoveProxy => {
                if let Some(a) = self.view.selected_asset {
                    self.remove_proxy(a);
                }
            }
            Action::LinkMedia => self.requests.extend(self.view.selected_asset.map(HostRequest::RelinkMedia)),
            Action::Export => self.show_export = true,
            Action::Undo => self.engine.undo(),
            Action::Redo => self.engine.redo(),
            Action::Speed => self.open_speed_dialog(),
            Action::FrameHold => self.frame_hold(),
            Action::Nest => self.open_nest_dialog(),
            Action::Channels => self.open_channels_dialog(),
            Action::AddEdit => {
                if let Some(seq) = self.active_seq() {
                    let targets: Vec<TrackId> = self.view.targeted.iter().copied().collect();
                    let r = edit::add_edit(self.snap(), seq.id, &targets, self.playhead);
                    self.run_edit(r);
                }
            }
            Action::VideoTransition => self.apply_default_transition(TrackKind::Video),
            Action::AudioTransition => self.apply_default_transition(TrackKind::Audio),
            Action::ToggleProxies => self.toggle_proxies(),
            Action::OpenSequence(id) => self.open_sequence(*id),
            Action::MarkIn => self.mark_in(),
            Action::MarkOut => self.mark_out(),
            Action::ClearInOut => self.clear_in_out(),
            Action::AddMarker => self.add_marker(),
            Action::NextMarker => self.go_to_marker(true),
            Action::PrevMarker => self.go_to_marker(false),
            Action::EditMarker => {
                if let Some(id) = self.view.selected_marker {
                    self.edit_marker(id);
                }
            }
            Action::NewTitle => self.new_generator(intrinsic::TITLE, self.playhead, None),
            Action::NewColorMatte => self.new_generator(intrinsic::COLOR_MATTE, self.playhead, None),
            Action::NewBars => self.new_generator(intrinsic::BARS, self.playhead, None),
            Action::TogglePanel(tab) => dock::toggle_tab(&mut self.dock, *tab),
            Action::ResetWorkspace => dock::reset(&mut self.dock, self.touch),
        }
    }

    /// The menus drawn in the window, when the host has no system menu bar.
    pub(crate) fn menu_bar(&mut self, ui: &mut Ui) {
        let platform = Platform::current();
        let mut chosen = None;
        for m in self.menus() {
            ui.menu_button(&m.title, |ui| {
                for e in &m.entries {
                    match e {
                        Entry::Separator => ui.menu_separator(),
                        Entry::Item(it) => {
                            let hint = it.shortcut.map(|c| libgui_keymap::format(c.resolve(platform), platform));
                            let label = match it.checked {
                                Some(true) => format!("✓ {}", it.label),
                                Some(false) => format!("    {}", it.label),
                                None => it.label.clone(),
                            };
                            if ui.menu_item_ex(&label, hint.as_deref(), it.enabled).clicked {
                                chosen = Some(it.action.clone());
                            }
                        }
                    }
                }
            });
        }
        if let Some(a) = chosen {
            self.perform(&a);
        }
    }
}
