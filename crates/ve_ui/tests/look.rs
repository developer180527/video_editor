//! Renders the editor to PNGs with libgui's CPU renderer, so the layout can be
//! looked at (and diffed) without a window or a GPU. Writes into
//! `target/ui-look/`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use libgui::*;
use libgui_soft::SoftRenderer;
use ve_engine::{edit, make_clip, Command, Engine, EngineClient};
use ve_model::*;
use ve_time::{Rate, Time};
use ve_ui::EditorUi;

const FONT: &[u8] = include_bytes!("../../../third_party/libgui/assets/Inter.ttf");

fn asset(name: &str, secs: i64, video: bool, audio: bool) -> Asset {
    Asset {
        id: AssetId::new(),
        name: name.into(),
        media: MediaRef(format!("file:/media/{name}")),
        info: Some(MediaInfo {
            duration: Time::from_seconds(secs),
            video: video.then(|| VideoStreamInfo { width: 1920, height: 1080, rate: Rate::FPS_24, codec: "h264".into() }),
            audio: audio.then(|| AudioStreamInfo { sample_rate: 48000, channels: 2, codec: "aac".into(), layout: "stereo".into() }).into_iter().collect(),
        }),
        variants: Vec::new(),
        marks: Default::default(),
    }
}

fn wait(client: &EngineClient, f: impl Fn(&EngineClient) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !f(client) {
        assert!(Instant::now() < deadline, "engine did not catch up");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// An engine holding a small hiking edit like the reference screenshot.
fn editor() -> EditorUi {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("look");
    let mut e = Engine::new(platform_headless::platform(&dir, Arc::new(platform_headless::NoMedia)));
    e.new_project("Hiking");
    let assets = [
        asset("DockAtitlanTL.mp4", 60, true, true),
        asset("AdobeStock_1166400934.mp4", 245, true, false),
        asset("Tikal.mp4", 22, true, true),
        asset("AntiguaArchTL.mp4", 134, true, true),
        asset("Atitlan.mp4", 48, true, true),
        asset("Music_Bed.wav", 211, false, true),
    ];
    for a in &assets {
        e.execute(Command::AddAsset { asset: Arc::new(a.clone()) }).unwrap();
    }
    let seq = e.snapshot().active().unwrap().clone();
    let (v1, v2, a1, a2) = (seq.tracks[0].id, seq.tracks[1].id, seq.tracks[2].id, seq.tracks[3].id);
    let s = Time::from_seconds;
    let mut place = |a: &Asset, at: i64, len: i64, vt: Option<TrackId>, at_track: Option<TrackId>| {
        let link = (vt.is_some() && at_track.is_some()).then(LinkId::new);
        let snap = e.snapshot();
        let mut items = Vec::new();
        if let Some(t) = vt {
            items.push((t, Arc::new(make_clip(e.plugins(), &seq.format, a, TrackKind::Video, s(len), link))));
        }
        if let Some(t) = at_track {
            items.push((t, Arc::new(make_clip(e.plugins(), &seq.format, a, TrackKind::Audio, s(len), link))));
        }
        let cmd = edit::overwrite(&snap, seq.id, s(at), &items).unwrap();
        e.execute(cmd).unwrap();
    };
    place(&assets[2], 0, 6, Some(v1), Some(a1));
    place(&assets[4], 6, 2, Some(v1), Some(a1));
    place(&assets[3], 8, 7, Some(v1), Some(a1));
    place(&assets[0], 3, 5, Some(v2), None);
    place(&assets[5], 0, 14, None, Some(a2));

    // The features on show: a dissolve on the cut at 6 s, a fade out of the
    // music, the last shot at half speed, a title on V2, in/out and markers.
    let snap = e.snapshot();
    let v1_clips = snap.active().unwrap().tracks[0].clips.clone();
    let tr = |id: &str, before: Time, after: Time| Transition {
        id: EffectId::new(),
        plugin: ve_engine::intrinsic::plugin_ref(id),
        before,
        after,
        params: Default::default(),
    };
    let half = Time::from_seconds_f64(0.5);
    e.execute(edit::set_transition(&snap, v1_clips[1].id, ve_engine::Edge::Start, Some(tr(ve_engine::intrinsic::DISSOLVE, half, half))).unwrap()).unwrap();
    let snap = e.snapshot();
    let music = snap.active().unwrap().tracks[3].clips[0].id;
    e.execute(edit::set_transition(&snap, music, ve_engine::Edge::End, Some(tr(ve_engine::intrinsic::CROSSFADE, s(2), Time::ZERO))).unwrap()).unwrap();
    let snap = e.snapshot();
    for id in edit::linked(&snap, v1_clips[2].id) {
        let snap = e.snapshot();
        e.execute(edit::set_speed(&snap, id, Ratio::new(1, 2), false).unwrap()).unwrap();
    }
    let title = ve_engine::make_generator_clip(e.plugins(), &seq.format, &ve_engine::intrinsic::plugin_ref(ve_engine::intrinsic::TITLE), s(4)).unwrap();
    let snap = e.snapshot();
    e.execute(edit::overwrite(&snap, seq.id, s(10), &[(v2, Arc::new(title))]).unwrap()).unwrap();
    let mut marks = Marks { in_point: Some(s(1)), out_point: Some(s(12)), ..Default::default() };
    for (t, name, color) in [(4, "Pan starts", MarkerColor::Green), (9, "Music swell", MarkerColor::Orange)] {
        marks.markers.push_back(Marker {
            id: MarkerId::new(),
            time: s(t),
            duration: Time::ZERO,
            name: name.into(),
            comment: String::new(),
            color,
            kind: MarkerKind::Comment,
        });
    }
    e.execute(Command::SetMarks { owner: ve_engine::MarksOwner::Sequence(seq.id), marks }).unwrap();
    e.seek(Time::from_seconds(2));
    let client = e.spawn(Arc::new(|| {}));
    wait(&client, |c| c.snapshot().active().is_some_and(|s| s.duration() > Time::ZERO));
    let mut ui = EditorUi::new(client, false);
    ui.view.selected_asset = Some(assets[1].id);
    ui
}

fn render(app: &mut EditorUi, w: u32, h: u32, name: &str, setup: impl Fn(&mut EditorUi)) {
    render_framed(app, w, h, name, ve_ui::WindowFrame::default(), setup);
}

fn render_framed(app: &mut EditorUi, w: u32, h: u32, name: &str, frame: ve_ui::WindowFrame, setup: impl Fn(&mut EditorUi)) {
    render_surface(app, w, h, name, SurfaceId::MAIN, frame, setup);
}

fn render_surface(app: &mut EditorUi, w: u32, h: u32, name: &str, surface: SurfaceId, frame: ve_ui::WindowFrame, setup: impl Fn(&mut EditorUi)) {
    let mut ui = Ui::new(ve_ui::theme(), FONT).expect("font");
    ui.reserve(8_000);
    let info = FrameInfo { screen_size: Vec2::new(w as f32, h as f32), scale: 1.0, dt: 1.0 / 60.0 };
    for _ in 0..4 {
        ui.begin_frame(info);
        app.ui_framed(&mut ui, surface, frame);
        let _ = ui.end_frame();
    }
    setup(app);
    for _ in 0..3 {
        ui.begin_frame(info);
        app.ui_framed(&mut ui, surface, frame);
        let _ = ui.end_frame();
    }
    ui.begin_frame(info);
    app.ui_framed(&mut ui, surface, frame);
    let out = ui.end_frame();
    let img = SoftRenderer::new().render_to_image(&out, w, h);
    drop(out);
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-look");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    let file = std::fs::File::create(&path).expect("create");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), img.width, img.height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&img.data).unwrap();
    println!("{}", path.display());
    // Everything the frame showed, cached subtrees and modals included.
    let nodes = ui.frame_cost().described_nodes();
    let min = if surface == SurfaceId::MAIN { 120 } else { 20 };
    assert!(nodes > min, "the window did not build ({nodes} nodes)");
}

#[test]
fn desktop_editor() {
    let mut app = editor();
    render(&mut app, 2000, 1129, "editor.png", |app| {
        // Select the first clip, as the reference has Effect Controls showing it.
        let snap = app.engine.snapshot();
        let first = snap.active().unwrap().tracks[0].clips[0].id;
        app.view.selection = edit::linked(&snap, first);
    });
}

/// The feature UI: a selected transition in Effect Controls, and the
/// Speed/Duration dialog over it all.
#[test]
fn features() {
    let mut app = editor();
    render(&mut app, 2000, 1129, "features.png", |app| {
        let snap = app.engine.snapshot();
        let cut = snap.active().unwrap().tracks[0].clips[1].id;
        app.view.selected_transition = Some((cut, ve_engine::Edge::Start));
        app.view.selection = vec![snap.active().unwrap().tracks[0].clips[2].id];
        app.view.dialog = Some(ve_ui::Dialog::Speed { clip: app.view.selection[0], percent: 50.0, reverse: false, ripple: true });
        app.view.selection.clear();
    });
}

/// The title bar as the editor draws it: the window buttons it draws
/// itself (Windows, Linux), and room left for the OS's (macOS).
#[test]
fn title_bars() {
    let mut app = editor();
    let drawn = ve_ui::WindowFrame { controls: ve_ui::WindowControls::Drawn { maximized: false }, system_menu: false };
    render_framed(&mut app, 2000, 1129, "titlebar-drawn.png", drawn, |_| {});
    let mac = ve_ui::WindowFrame { controls: ve_ui::WindowControls::Leading { inset: 78.0 }, system_menu: true };
    render_framed(&mut app, 2000, 1129, "titlebar-mac.png", mac, |_| {});
    // Every panel is listed in View, and toggling one closes and reopens it.
    let view = |app: &EditorUi| app.menus().into_iter().find(|m| m.title == "View").unwrap();
    let checked = |app: &EditorUi, name: &str| {
        view(app).entries.iter().any(|e| matches!(e, ve_ui::Entry::Item(i) if i.label == name && i.checked == Some(true)))
    };
    assert!(checked(&app, "Program"));
    app.perform(&ve_ui::Action::TogglePanel(ve_ui::Tab::Program));
    assert!(!checked(&app, "Program"), "closed");
    app.perform(&ve_ui::Action::TogglePanel(ve_ui::Tab::Program));
    assert!(checked(&app, "Program"), "back");
    app.perform(&ve_ui::Action::ResetWorkspace);
    assert!(checked(&app, "Timeline"));
}

/// Torn-off windows with the editor's own title bar (Windows, Linux): the
/// tab bar is the title bar, taller than a docked one, with the window
/// buttons at its right end.
#[test]
fn torn_off_windows() {
    let mut app = editor();
    let drawn = ve_ui::WindowFrame { controls: ve_ui::WindowControls::Drawn { maximized: false }, system_menu: false };
    let floating = |app: &EditorUi, tab: ve_ui::Tab| {
        app.dock()
            .surfaces()
            .iter()
            .find(|s| s.floating && s.first_tab() == Some(&tab))
            .map(|s| s.id)
            .expect("a window for the panel")
    };
    // As if torn off: out of the main window, into one of its own.
    app.perform(&ve_ui::Action::TogglePanel(ve_ui::Tab::Program));
    app.open_floating(ve_ui::Tab::Program, Vec2::new(960.0, 600.0));
    let program = floating(&app, ve_ui::Tab::Program);
    render_surface(&mut app, 960, 600, "torn-off-program.png", program, drawn, |_| {});
    app.open_floating(ve_ui::Tab::Settings, Vec2::new(560.0, 520.0));
    let settings = floating(&app, ve_ui::Tab::Settings);
    render_surface(&mut app, 560, 520, "settings-window.png", settings, drawn, |_| {});
    assert_eq!(app.title_strip(settings), ve_ui::FLOATING_TAB_HEIGHT, "a torn-off title bar is the tall one");
    // The layout survives a round trip, torn-off windows included.
    let saved = app.layout_toml().expect("layout");
    app.perform(&ve_ui::Action::ResetWorkspace);
    assert!(app.dock().surfaces().len() == 1);
    assert!(app.restore_layout(&saved));
    assert_eq!(app.dock().surfaces().len(), 3);
}

/// The Color, Scopes and Audio Track Mixer panels, each brought to the
/// front of its pane.
#[test]
fn color_scopes_and_mixer_panels() {
    let mut app = editor();
    let show = |app: &mut EditorUi, tab: ve_ui::Tab| {
        let at = app.dock().find_tab(|t| *t == tab).expect("panel in the layout");
        app.dock_mut().focus_tab(at);
    };
    render(&mut app, 2000, 1129, "panel-color.png", |app| {
        show(app, ve_ui::Tab::Color);
        let snap = app.engine.snapshot();
        app.view.selection = vec![snap.active().unwrap().tracks[0].clips[0].id];
    });
    render(&mut app, 2000, 1129, "panel-scopes.png", |app| {
        show(app, ve_ui::Tab::Scopes);
        app.view.scope = ve_render::scopes::ScopeKind::Vectorscope;
    });
    render(&mut app, 2000, 1129, "panel-mixer.png", |app| show(app, ve_ui::Tab::TrackMixer));
    // The Source monitor with a clip marked.
    let snap = app.engine.snapshot();
    let tikal = snap.assets.values().find(|a| a.name == "Tikal.mp4").unwrap().id;
    let marks = Marks { in_point: Some(Time::from_seconds(4)), out_point: Some(Time::from_seconds(12)), ..Default::default() };
    app.engine.execute(Command::SetMarks { owner: ve_engine::MarksOwner::Asset(tikal), marks });
    app.engine.set_source(tikal);
    wait(&app.engine, |c| c.source().is_some());
    render(&mut app, 2000, 1129, "panel-source.png", |app| show(app, ve_ui::Tab::Source));
}

fn frame(ui: &mut Ui, app: &mut EditorUi) {
    let info = FrameInfo { screen_size: Vec2::new(2000.0, 1129.0), scale: 1.0, dt: 1.0 / 60.0 };
    ui.begin_frame(info);
    app.ui(ui);
    let _ = ui.end_frame();
}

fn clip_on(app: &EditorUi, track: usize, i: usize) -> Arc<Clip> {
    app.engine.snapshot().active().unwrap().tracks[track].clips[i].clone()
}

/// Drive the real UI with pointer events: drag a clip on V2, then razor one
/// on V1, then undo — through libgui input, the tool preview, the engine
/// thread and back.
#[test]
fn drag_and_razor_through_the_ui() {
    let mut app = editor();
    let mut ui = Ui::new(ve_ui::theme(), FONT).expect("font");
    for _ in 0..4 {
        frame(&mut ui, &mut app);
    }
    let before = clip_on(&app, 1, 0); // DockAtitlan on V2, 3 s – 8 s
    assert_eq!(before.timeline_start, Time::from_seconds(3));

    // Same layout as editor.png: V2 lane at y ≈ 815, 78 px per second from x ≈ 831.
    let at = |s: f32| 831.0 + s * 78.0;
    let mv = |ui: &mut Ui, x: f32, y: f32| ui.push(InputEvent::PointerMoved { pos: Vec2::new(x, y) });
    let button = |ui: &mut Ui, down: bool| ui.push(InputEvent::PointerButton { button: PointerButton::Primary, pressed: down });

    mv(&mut ui, at(5.0), 815.0);
    frame(&mut ui, &mut app);
    button(&mut ui, true);
    frame(&mut ui, &mut app);
    for k in 1..=10 {
        mv(&mut ui, at(5.0) - 10.0 * k as f32, 815.0); // 100 px left ≈ 1.28 s
        frame(&mut ui, &mut app);
    }
    button(&mut ui, false);
    frame(&mut ui, &mut app);
    wait(&app.engine, |c| c.published().undo_label.as_deref() == Some("Move"));
    let moved = clip_on(&app, 1, 0);
    let secs = moved.timeline_start.as_seconds_f64();
    assert!((1.6..1.9).contains(&secs), "moved to {secs}");
    assert_eq!(moved.timeline_start, moved.timeline_start.round_to_frame(Rate::FPS_24), "lands on a frame");

    // Razor (C) on V1 at 3 s: Tikal splits, and its linked audio with it.
    ui.push(InputEvent::Key { key: Key::C, pressed: true, repeat: false });
    frame(&mut ui, &mut app);
    ui.push(InputEvent::Key { key: Key::C, pressed: false, repeat: false });
    mv(&mut ui, at(3.0), 870.0);
    frame(&mut ui, &mut app);
    button(&mut ui, true);
    frame(&mut ui, &mut app);
    button(&mut ui, false);
    frame(&mut ui, &mut app);
    wait(&app.engine, |c| c.published().undo_label.as_deref() == Some("Razor"));
    let seq = app.engine.snapshot().active().unwrap().clone();
    assert_eq!(seq.tracks[0].clips.len(), 4, "V1: Tikal split in two");
    assert_eq!(seq.tracks[2].clips.len(), 4, "A1: its audio split with it");

    // Undo puts it back.
    app.engine.undo();
    wait(&app.engine, |c| c.published().undo_label.as_deref() == Some("Move"));
    assert_eq!(app.engine.snapshot().active().unwrap().tracks[0].clips.len(), 3);
}

fn press(ui: &mut Ui, app: &mut EditorUi, key: Key, mods: Modifiers) {
    ui.push(InputEvent::ModifiersChanged(mods));
    ui.push(InputEvent::Key { key, pressed: true, repeat: false });
    frame(ui, app);
    ui.push(InputEvent::Key { key, pressed: false, repeat: false });
    ui.push(InputEvent::ModifiersChanged(Modifiers::NONE));
    frame(ui, app);
}

/// The marking and transition shortcuts, through the real UI: I and O set
/// in and out, M drops a marker, Cmd+D puts a dissolve on the nearest cut,
/// and Delete takes the selected transition off again.
#[test]
fn marks_and_transitions_from_the_keyboard() {
    let mut app = editor();
    let mut ui = Ui::new(ve_ui::theme(), FONT).expect("font");
    for _ in 0..4 {
        frame(&mut ui, &mut app);
    }
    let marks = |app: &EditorUi| app.engine.snapshot().active().unwrap().marks.clone();
    let markers_before = marks(&app).markers.len();

    app.engine.seek(Time::from_seconds(3));
    frame(&mut ui, &mut app);
    press(&mut ui, &mut app, Key::I, Modifiers::NONE);
    wait(&app.engine, |_| marks(&app).in_point == Some(Time::from_seconds(3)));
    press(&mut ui, &mut app, Key::M, Modifiers::NONE);
    wait(&app.engine, |_| marks(&app).markers.len() == markers_before + 1);

    // Out at 5 s is the end of the frame under the playhead.
    app.engine.seek(Time::from_seconds(5));
    frame(&mut ui, &mut app);
    press(&mut ui, &mut app, Key::O, Modifiers::NONE);
    let out = Time::from_seconds(5) + Rate::FPS_24.frame_to_time(1);
    wait(&app.engine, |_| marks(&app).out_point == Some(out));

    // Cmd+D at 5 s: the nearest edit point on V1 is the cut at 6 s, which
    // already has a dissolve from the fixture — so first take that off.
    let cut = clip_on(&app, 0, 1);
    assert!(cut.transition_in.is_some());
    app.view.selected_transition = Some((cut.id, ve_engine::Edge::Start));
    press(&mut ui, &mut app, Key::Delete, Modifiers::NONE);
    wait(&app.engine, |_| clip_on(&app, 0, 1).transition_in.is_none());
    press(&mut ui, &mut app, Key::D, Modifiers { logo: true, ..Modifiers::NONE });
    wait(&app.engine, |_| clip_on(&app, 0, 1).transition_in.is_some());
    let t = clip_on(&app, 0, 1).transition_in.clone().unwrap();
    assert_eq!(t.plugin.id, ve_engine::intrinsic::DISSOLVE);
    assert_eq!(t.before + t.after, Time::from_seconds(1), "one second, centred");
    assert_eq!(t.before, t.after);
}

/// Three-point editing through the UI: open a clip in the Source monitor,
/// mark it with I and O, shuttle with L, then cut it in with comma.
#[test]
fn source_monitor_three_point_insert() {
    let mut app = editor();
    let mut ui = Ui::new(ve_ui::theme(), FONT).expect("font");
    for _ in 0..4 {
        frame(&mut ui, &mut app);
    }
    let snap = app.engine.snapshot();
    let atitlan = snap.assets.values().find(|a| a.name == "Atitlan.mp4").unwrap().id;
    app.engine.set_source(atitlan);
    wait(&app.engine, |c| c.viewer() == ve_engine::Viewer::Source);
    frame(&mut ui, &mut app);
    let s = Time::from_seconds;

    // Marks go on the clip, not the sequence.
    app.engine.seek(s(2));
    frame(&mut ui, &mut app);
    press(&mut ui, &mut app, Key::I, Modifiers::NONE);
    app.engine.seek(s(5));
    frame(&mut ui, &mut app);
    press(&mut ui, &mut app, Key::O, Modifiers::NONE);
    let out = s(5) + Rate::FPS_24.frame_to_time(1);
    wait(&app.engine, |c| c.snapshot().assets.get(&atitlan).unwrap().marks.out_point.is_some());
    let marks = app.engine.snapshot().assets.get(&atitlan).unwrap().marks.clone();
    assert_eq!(marks.in_point, Some(s(2)));
    assert_eq!(marks.out_point, Some(out), "the end of the frame at 5 s");
    let seq_marks = app.engine.snapshot().active().unwrap().marks.clone();
    assert_ne!(seq_marks.in_point, Some(s(2)), "the sequence's marks are untouched");

    // L plays the source, L again doubles, K stops: the program stays put.
    let program = app.engine.playhead_of(ve_engine::Viewer::Program);
    press(&mut ui, &mut app, Key::L, Modifiers::NONE);
    assert!(matches!(app.engine.published().transport.state(), ve_engine::State::Playing { rate } if rate == 1.0));
    press(&mut ui, &mut app, Key::L, Modifiers::NONE);
    assert!(matches!(app.engine.published().transport.state(), ve_engine::State::Playing { rate } if rate == 2.0));
    press(&mut ui, &mut app, Key::K, Modifiers::NONE);
    assert!(!app.engine.is_playing());
    assert_eq!(app.engine.playhead_of(ve_engine::Viewer::Program), program);

    // Clear the sequence's in/out and park the program at 22 s (the end).
    app.engine.execute(Command::SetMarks { owner: ve_engine::MarksOwner::Sequence(app.engine.snapshot().active().unwrap().id), marks: Default::default() });
    app.engine.set_viewer(ve_engine::Viewer::Program);
    app.engine.seek(s(22));
    frame(&mut ui, &mut app);
    // Comma inserts the marked part at the program playhead.
    press(&mut ui, &mut app, Key::Comma, Modifiers::NONE);
    wait(&app.engine, |c| c.snapshot().active().unwrap().tracks[0].clips.len() == 4);
    let seq = app.engine.snapshot().active().unwrap().clone();
    let clip = seq.tracks[0].clips.iter().find(|c| c.timeline_start == s(22)).expect("cut in at the playhead");
    assert_eq!(clip.source_range, ve_time::TimeRange::new(s(2), out - s(2)), "just the marked part");
    let audio = seq.tracks[2].clips.iter().find(|c| c.timeline_start == s(22)).expect("with its sound");
    assert_eq!(audio.source_range, clip.source_range);
    // The program playhead parks after the new clip.
    frame(&mut ui, &mut app);
    assert_eq!(app.engine.playhead_of(ve_engine::Viewer::Program), s(22) + (out - s(2)));
}

