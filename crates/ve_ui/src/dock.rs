//! The editor body as a dock tree, arranged the way every NLE opens: source
//! and effect controls top-left, the program monitor top-right, the bin and
//! the timeline below.

use libgui::*;

use crate::{effects, program, project, timeline, EditorUi};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Source,
    EffectControls,
    AudioClipMixer,
    Metadata,
    Program,
    Project,
    Effects,
    MediaBrowser,
    Info,
    Timeline,
}

impl Tab {
    pub const ALL: [Tab; 10] = [
        Tab::Source,
        Tab::EffectControls,
        Tab::AudioClipMixer,
        Tab::Metadata,
        Tab::Program,
        Tab::Project,
        Tab::Effects,
        Tab::MediaBrowser,
        Tab::Info,
        Tab::Timeline,
    ];

    /// Identity in a saved layout: a name, never a position.
    pub fn key(self) -> u64 {
        Id::from_name(match self {
            Tab::Source => "source",
            Tab::EffectControls => "effect-controls",
            Tab::AudioClipMixer => "audio-clip-mixer",
            Tab::Metadata => "metadata",
            Tab::Program => "program",
            Tab::Project => "project",
            Tab::Effects => "effects",
            Tab::MediaBrowser => "media-browser",
            Tab::Info => "info",
            Tab::Timeline => "timeline",
        })
        .0
    }

    /// A short name, for a torn-off window's title bar.
    pub fn name(self) -> &'static str {
        match self {
            Tab::Source => "Source",
            Tab::EffectControls => "Effect Controls",
            Tab::AudioClipMixer => "Audio Clip Mixer",
            Tab::Metadata => "Metadata",
            Tab::Program => "Program",
            Tab::Project => "Project",
            Tab::Effects => "Effects",
            Tab::MediaBrowser => "Media Browser",
            Tab::Info => "Info",
            Tab::Timeline => "Timeline",
        }
    }

    pub fn from_key(key: u64) -> Option<Tab> {
        Tab::ALL.into_iter().find(|t| t.key() == key)
    }
}

pub(crate) fn initial(touch: bool) -> DockState<Tab> {
    let mut dock = DockState::new();
    let mut top_left = dock.leaf(vec![Tab::Source, Tab::EffectControls, Tab::AudioClipMixer, Tab::Metadata]);
    if let DockNode::Leaf(l) = &mut top_left {
        l.active = 1;
    }
    let top_right = dock.leaf(vec![Tab::Program]);
    let top = dock.split(Axis::X, if touch { 0.42 } else { 0.505 }, top_left, top_right);
    let bin = dock.leaf(vec![Tab::Project, Tab::Effects, Tab::MediaBrowser, Tab::Info]);
    let seq = dock.leaf(vec![Tab::Timeline]);
    let bottom = dock.split(Axis::X, 0.325, bin, seq);
    let root = dock.split(Axis::Y, if touch { 0.5 } else { 0.615 }, top, bottom);
    dock.set_root(SurfaceId::MAIN, root);
    dock
}

pub(crate) struct Viewer<'a> {
    pub app: &'a mut EditorUi,
}

impl Viewer<'_> {
    pub fn title_of(&self, tab: Tab) -> String {
        self.title(&tab)
    }
}

impl TabViewer for Viewer<'_> {
    type Tab = Tab;

    fn title(&self, tab: &Tab) -> String {
        let seq = self.app.snap().active().map(|s| s.name.clone()).unwrap_or_default();
        match tab {
            Tab::Source => "Source: (no clips)".into(),
            Tab::EffectControls => "Effect Controls".into(),
            Tab::AudioClipMixer => format!("Audio Clip Mixer: {seq}"),
            Tab::Metadata => "Metadata".into(),
            Tab::Program => format!("Program: {seq}"),
            Tab::Project => format!("Project: {}", self.app.snap().name),
            Tab::Effects => "Effects".into(),
            Tab::MediaBrowser => "Media Browser".into(),
            Tab::Info => "Info".into(),
            Tab::Timeline => seq,
        }
    }

    fn id(&self, tab: &Tab) -> u64 {
        tab.key()
    }

    fn scroll(&self, tab: &Tab) -> bool {
        matches!(tab, Tab::Metadata | Tab::MediaBrowser | Tab::Info | Tab::Effects)
    }

    fn padding(&self, tab: &Tab) -> Insets {
        match tab {
            Tab::Program | Tab::Timeline | Tab::EffectControls | Tab::Project => Insets::all(0.0),
            _ => Insets::all(10.0),
        }
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        match tab {
            Tab::EffectControls => effects::panel(ui, self.app),
            Tab::Program => program::panel(ui, self.app),
            Tab::Project => project::panel(ui, self.app),
            Tab::Timeline => timeline::panel(ui, self.app),
            Tab::Effects => project::effects_list(ui, self.app),
            Tab::Info => info(ui, self.app),
            other => placeholder(ui, &self.title(other)),
        }
    }
}

/// Panels not built yet: a name, so a dragged tab still shows something honest.
fn placeholder(ui: &mut Ui, title: &str) {
    let t = ui.theme.clone();
    ui.container(
        Layout::column().width(Size::Grow(1.0)).height(Size::Grow(1.0)).align(Align::Center, Align::Center),
        Frame::none(),
        |ui| ui.text_with(title, t.metrics.font_size, t.palette.text_faint),
    );
}

/// What the platform allows, and what the engine is doing.
fn info(ui: &mut Ui, app: &mut EditorUi) {
    let c = app.st.capabilities.clone();
    let yes = |b: bool| if b { "yes" } else { "no" };
    ui.heading(c.platform_name);
    ui.label(&format!("Child processes: {}", yes(c.processes)));
    ui.label(&format!("Plugins from files: {}", yes(c.dynamic_libraries)));
    ui.label(&format!("JIT: {}", yes(c.jit)));
    ui.label(&format!("Cache budget: {} MB", c.memory_budget >> 20));
    ui.label(&format!("Hardware decode: {}", c.hw_decode.join(", ")));
    ui.space(8.0);
    if let Some(seq) = app.snap().active() {
        let f = &seq.format;
        ui.heading(&seq.name);
        ui.label(&format!("{}×{} · {:.3} fps · {} Hz", f.width, f.height, f.rate.as_f64(), f.sample_rate));
        ui.label(&format!("Working space: {}", f.working_space));
        ui.label(&format!("Duration: {}", app.timecode(seq.duration())));
    }
}

/// Every panel open in any window.
pub(crate) fn open_tabs(dock: &DockState<Tab>) -> Vec<Tab> {
    fn walk(n: &DockNode<Tab>, out: &mut Vec<Tab>) {
        match n {
            DockNode::Leaf(l) => out.extend(l.tabs.iter().copied()),
            DockNode::Split(s) => {
                walk(&s.first, out);
                walk(&s.second, out);
            }
        }
    }
    let mut out = Vec::new();
    for s in dock.surfaces() {
        if let Some(r) = &s.root {
            walk(r, &mut out);
        }
    }
    out
}

/// `node` without `tab`; panes left empty go, and a split left with one
/// side becomes that side.
fn without(dock: &mut DockState<Tab>, node: DockNode<Tab>, tab: Tab) -> Option<DockNode<Tab>> {
    match node {
        DockNode::Leaf(mut l) => {
            l.tabs.retain(|t| *t != tab);
            if l.tabs.is_empty() {
                return None;
            }
            l.active = l.active.min(l.tabs.len() - 1);
            Some(DockNode::Leaf(l))
        }
        DockNode::Split(s) => {
            let (axis, fraction) = (s.axis, s.fraction);
            match (without(dock, *s.first, tab), without(dock, *s.second, tab)) {
                (Some(a), Some(b)) => Some(dock.split(axis, fraction, a, b)),
                (Some(a), None) | (None, Some(a)) => Some(a),
                (None, None) => None,
            }
        }
    }
}

/// Close `tab` where it is open; open it in the main window where it is not.
pub(crate) fn toggle_tab(dock: &mut DockState<Tab>, tab: Tab) {
    if !open_tabs(dock).contains(&tab) {
        dock.add_tab(SurfaceId::MAIN, tab);
        return;
    }
    let ids: Vec<SurfaceId> = dock.surfaces().iter().map(|s| s.id).collect();
    for id in ids {
        let Some(root) = dock.take_root(id) else { continue };
        match without(dock, root, tab) {
            Some(r) => dock.set_root(id, r),
            // A torn-off window with nothing left in it closes.
            None if id != SurfaceId::MAIN => dock.close_surface(id),
            None => {}
        }
    }
}

/// Back to the layout the editor opens with; torn-off windows close.
pub(crate) fn reset(dock: &mut DockState<Tab>, touch: bool) {
    let floating: Vec<SurfaceId> = dock.surfaces().iter().map(|s| s.id).filter(|id| *id != SurfaceId::MAIN).collect();
    for id in floating {
        dock.take_root(id);
        dock.close_surface(id);
    }
    let config = dock.config.clone();
    *dock = initial(touch);
    dock.config = config;
}
