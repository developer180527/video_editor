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
    e.seek(Time::from_seconds(2));
    let client = e.spawn(Arc::new(|| {}));
    wait(&client, |c| c.snapshot().active().is_some_and(|s| s.duration() > Time::ZERO));
    let mut ui = EditorUi::new(client, false);
    ui.view.selected_asset = Some(assets[1].id);
    ui
}

fn render(app: &mut EditorUi, w: u32, h: u32, name: &str, setup: impl Fn(&mut EditorUi)) {
    let mut ui = Ui::new(ve_ui::theme(), FONT).expect("font");
    ui.reserve(8_000);
    let info = FrameInfo { screen_size: Vec2::new(w as f32, h as f32), scale: 1.0, dt: 1.0 / 60.0 };
    for _ in 0..4 {
        ui.begin_frame(info);
        app.ui(&mut ui);
        let _ = ui.end_frame();
    }
    setup(app);
    for _ in 0..3 {
        ui.begin_frame(info);
        app.ui(&mut ui);
        let _ = ui.end_frame();
    }
    ui.begin_frame(info);
    app.ui(&mut ui);
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
    assert!(ui.frame_cost().nodes > 200, "the editor did not build");
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
