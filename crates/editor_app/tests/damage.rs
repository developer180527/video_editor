//! The window repaints only what changed (platform_winit::damage). Here the
//! editor's real frames are rendered whole on the CPU before and after a
//! change, and every pixel that differs must lie inside the damage the
//! tracker reports — anything outside it would stay stale on screen.

use std::sync::Arc;
use std::time::{Duration, Instant};

use libgui::*;
use libgui_soft::SoftRenderer;
use platform_winit::damage::{Damage, Tracker};
use ve_engine::{edit, make_clip, Command, Engine, EngineClient};
use ve_model::*;
use ve_time::{Rate, Time};
use ve_ui::EditorUi;

const FONT: &[u8] = include_bytes!("../../../third_party/libgui/assets/Inter.ttf");
const SIZE: (u32, u32) = (1600, 900);

fn asset(name: &str, secs: i64, video: bool, audio: bool) -> Asset {
    Asset {
        id: AssetId::new(),
        name: name.into(),
        media: MediaRef(format!("file:/media/{name}")),
        info: Some(MediaInfo {
            duration: Time::from_seconds(secs),
            video: video.then(|| VideoStreamInfo::new(1920, 1080, Rate::FPS_24, "h264")),
            audio: audio.then(|| AudioStreamInfo { sample_rate: 48000, channels: 2, codec: "aac".into(), layout: "stereo".into(), decodable: true }).into_iter().collect(),
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

fn editor() -> EditorUi {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("damage");
    let mut platform = platform_headless::platform(&dir, Arc::new(platform_headless::NoMedia));
    platform.libraries = Arc::new(platform_headless::LinkedOnly { make: ve_builtins::linked() });
    let mut e = Engine::new(platform);
    e.new_project("Damage");
    let assets = [asset("A.mp4", 30, true, true), asset("B.mp4", 20, true, true), asset("Music.wav", 60, false, true)];
    for a in &assets {
        e.execute(Command::AddAsset { asset: Arc::new(a.clone()) }).unwrap();
    }
    let seq = e.snapshot().active().unwrap().clone();
    let (v1, a1, a2) = (seq.tracks[0].id, seq.tracks[2].id, seq.tracks[3].id);
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
        e.execute(edit::overwrite(&snap, seq.id, s(at), &items).unwrap()).unwrap();
    };
    place(&assets[0], 0, 6, Some(v1), Some(a1));
    place(&assets[1], 6, 5, Some(v1), Some(a1));
    place(&assets[2], 0, 11, None, Some(a2));
    e.seek(s(2));
    let client = e.spawn(Arc::new(|| {}));
    wait(&client, |c| c.snapshot().active().is_some_and(|s| s.duration() > Time::ZERO));
    EditorUi::new(client, false)
}

struct Shown {
    image: Vec<u8>,
    damage: Damage,
}

/// Build frames until the UI settles, then the frame shown: its pixels and
/// its damage against the last frame the tracker saw.
fn frame(app: &mut EditorUi, ui: &mut Ui, tracker: &mut Tracker, events: Vec<InputEvent>) -> Shown {
    let info = FrameInfo { screen_size: Vec2::new(SIZE.0 as f32, SIZE.1 as f32), scale: 1.0, dt: 1.0 / 60.0 };
    for e in events {
        ui.push(e);
    }
    // A few builds settle hover and layout; each is one frame on screen, so
    // the damage is accumulated over all of them.
    let mut rects: Option<Vec<[u32; 4]>> = Some(Vec::new());
    let mut image = Vec::new();
    for _ in 0..4 {
        ui.begin_frame(info);
        app.ui_framed(ui, SurfaceId::MAIN, ve_ui::WindowFrame::default());
        let out = ui.end_frame();
        let c = out.clear_color;
        match tracker.diff(&out.draw.instances, &out.draw.batches, Some(&[]), SIZE, 1.0, [c.r, c.g, c.b, 1.0]) {
            Damage::All => rects = None,
            Damage::Rects(r) => {
                if let Some(v) = rects.as_mut() {
                    v.extend(r)
                }
            }
            Damage::Nothing => {}
        }
        image = SoftRenderer::new().render_to_image(&out, SIZE.0, SIZE.1).data;
    }
    let damage = match rects {
        None => Damage::All,
        Some(r) if r.is_empty() => Damage::Nothing,
        Some(r) => Damage::Rects(r),
    };
    Shown { image, damage }
}

/// Every changed pixel inside the damage; returns how much of the window
/// the damage covered.
fn covered(before: &Shown, after: &Shown, what: &str) -> f64 {
    let rects = match &after.damage {
        Damage::All => return 1.0,
        Damage::Nothing => Vec::new(),
        Damage::Rects(r) => r.clone(),
    };
    let mut missed = 0;
    let mut first = None;
    for y in 0..SIZE.1 {
        for x in 0..SIZE.0 {
            let i = ((y * SIZE.0 + x) * 4) as usize;
            if before.image[i..i + 4] != after.image[i..i + 4] && !rects.iter().any(|r| x >= r[0] && x < r[2] && y >= r[1] && y < r[3]) {
                missed += 1;
                first.get_or_insert((x, y));
            }
        }
    }
    assert_eq!(missed, 0, "{what}: {missed} changed pixels outside the damage, the first at {first:?}; damage {rects:?}");
    let area: u64 = rects.iter().map(|r| (r[2] - r[0]) as u64 * (r[3] - r[1]) as u64).sum();
    area as f64 / (SIZE.0 as f64 * SIZE.1 as f64)
}

#[test]
fn every_change_on_screen_is_repainted() {
    let mut app = editor();
    let mut ui = Ui::new(ve_ui::theme(), FONT).expect("font");
    ui.reserve(8_000);
    let mut tracker = Tracker::default();
    let _ = frame(&mut app, &mut ui, &mut tracker, Vec::new());
    // Settled: the same frame again is no damage at all.
    let mut last = frame(&mut app, &mut ui, &mut tracker, Vec::new());
    assert_eq!(last.damage, Damage::Nothing, "an unchanged frame repaints nothing");

    let moves: Vec<(&str, Box<dyn Fn(&mut EditorUi) -> Vec<InputEvent>>)> = vec![
        ("playhead moved", Box::new(|app: &mut EditorUi| {
            app.engine.seek(Time::from_seconds(7));
            wait(&app.engine, |c| c.playhead() == Time::from_seconds(7));
            Vec::new()
        })),
        ("pointer over the timeline", Box::new(|_: &mut EditorUi| vec![InputEvent::PointerMoved { pos: Vec2::new(500.0, 700.0) }])),
        ("pointer over the bin", Box::new(|_: &mut EditorUi| vec![InputEvent::PointerMoved { pos: Vec2::new(120.0, 200.0) }])),
        ("pointer gone", Box::new(|_: &mut EditorUi| vec![InputEvent::PointerLeft])),
        ("a clip selected", Box::new(|app: &mut EditorUi| {
            let snap = app.engine.snapshot();
            let first = snap.active().unwrap().tracks[0].clips[0].id;
            app.view.selection = edit::linked(&snap, first);
            Vec::new()
        })),
        ("timeline scrolled", Box::new(|_: &mut EditorUi| vec![InputEvent::PointerMoved { pos: Vec2::new(600.0, 720.0) }, InputEvent::Wheel { delta: Vec2::new(0.0, 3.0), unit: WheelUnit::Line }])),
        ("a dialog opened", Box::new(|app: &mut EditorUi| {
            let clip = app.engine.snapshot().active().unwrap().tracks[0].clips[1].id;
            app.view.dialog = Some(ve_ui::Dialog::Speed { clip, percent: 50.0, reverse: false, ripple: true });
            Vec::new()
        })),
        ("the dialog closed", Box::new(|app: &mut EditorUi| {
            app.view.dialog = None;
            Vec::new()
        })),
    ];
    for (what, change) in moves {
        let events = change(&mut app);
        let now = frame(&mut app, &mut ui, &mut tracker, events);
        let share = covered(&last, &now, what);
        eprintln!("{what}: {:.1}% of the window repainted", share * 100.0);
        last = now;
    }
}
