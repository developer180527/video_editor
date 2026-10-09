//! The model completion features end to end: evaluate → resolve →
//! compositor on a real GPU. Generators give exact pictures, so most need no
//! media files. Skips without a GPU (or the `ffmpeg` CLI where media is made).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use ve_engine::*;
use ve_model::*;
use ve_render::Compositor;
use ve_time::Time;

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

fn tmp(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// An engine on a small 64x36 sequence (V1 V2 A1 A2).
fn engine(dir: &str) -> Engine {
    let mut e = Engine::new(platform_headless::platform(tmp(dir), Arc::new(media_ffmpeg::Ffmpeg::new())));
    e.new_project("t");
    let snap = e.snapshot();
    let mut seq = (**snap.active().unwrap()).clone();
    seq.format.width = 64;
    seq.format.height = 36;
    e.execute(Command::RemoveSequence { sequence: seq.id }).unwrap();
    e.execute(Command::AddSequence { sequence: Arc::new(seq.clone()) }).unwrap();
    e.execute(Command::SetActiveSequence { sequence: Some(seq.id) }).unwrap();
    e
}

fn matte(e: &Engine, rgb: [f32; 3], start: i64, len: i64) -> Clip {
    let seq = e.snapshot().active().unwrap().clone();
    let mut c = make_generator_clip(e.plugins(), &seq.format, &intrinsic::plugin_ref(intrinsic::COLOR_MATTE), Time::from_seconds(len)).unwrap();
    let g = Arc::make_mut(&mut c.effects[0]);
    g.params.insert("color".into(), Param::Constant(Value::Color([rgb[0], rgb[1], rgb[2], 1.0])));
    c.timeline_start = Time::from_seconds(start);
    c
}

fn put(e: &mut Engine, track: usize, c: Clip) -> ClipId {
    let seq = e.snapshot().active().unwrap().clone();
    let id = c.id;
    e.execute(Command::AddClip { sequence: seq.id, track: seq.tracks[track].id, clip: Arc::new(c) }).unwrap();
    id
}

/// The monitor at `t` (seconds), as RGBA8 rows, and a pixel reader.
fn shot(e: &Engine, gpu: &(wgpu::Device, wgpu::Queue), t: f64, proxies: bool) -> (u32, Vec<u8>) {
    let snap = e.snapshot();
    let seq = snap.active().unwrap();
    let plan = ve_render::evaluate(seq, Time::from_seconds_f64(t), Quality { scale: 1.0, use_proxies: proxies });
    let frame = ve_engine::frame::resolve(&snap, plan, e.plugins(), e.video(), Some(Duration::from_secs(5)));
    assert!(frame.missing.is_empty(), "{:?}", frame.missing);
    let mut c = Compositor::new(&gpu.0);
    c.render(&gpu.0, &gpu.1, &frame.plan, &frame.layers, frame.seq_size, frame.space);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let (w, _, px) = c.read_output(&gpu.0, &gpu.1).unwrap();
    (w, px)
}

fn px(img: &(u32, Vec<u8>), x: u32, y: u32) -> [u8; 3] {
    let i = ((y * img.0 + x) * 4) as usize;
    [img.1[i], img.1[i + 1], img.1[i + 2]]
}

fn near(a: [u8; 3], b: [u8; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 2)
}

#[test]
fn generators_draw() {
    let Some(g) = gpu() else { return eprintln!("skipped: no GPU") };
    let mut e = engine("gen");
    { let c = matte(&e, [1.0, 0.0, 0.0], 0, 2); put(&mut e, 0, c) };
    let seq = e.snapshot().active().unwrap().clone();
    let mut bars = make_generator_clip(e.plugins(), &seq.format, &intrinsic::plugin_ref(intrinsic::BARS), Time::from_seconds(2)).unwrap();
    bars.timeline_start = Time::from_seconds(2);
    put(&mut e, 0, bars);
    let mut title = make_generator_clip(e.plugins(), &seq.format, &intrinsic::plugin_ref(intrinsic::TITLE), Time::from_seconds(2)).unwrap();
    let p = Arc::make_mut(&mut title.effects[0]);
    p.params.insert("text".into(), Param::Constant(Value::Text("H".into())));
    p.params.insert("size".into(), Param::Constant(Value::Float(30.0)));
    p.params.insert("position".into(), Param::Constant(Value::Vec2([32.0, 18.0])));
    title.timeline_start = Time::from_seconds(4);
    put(&mut e, 0, title);

    assert!(near(px(&shot(&e, &g, 1.0, false), 32, 18), [255, 0, 0]), "a red matte is red");
    assert!(near(px(&shot(&e, &g, 3.0, false), 4, 5), [191, 191, 191]), "75% white bar");
    let t = shot(&e, &g, 5.0, false);
    let inked = t.1.chunks(4).filter(|p| p[0] > 200).count();
    assert!(inked > 20 && px(&t, 1, 1) == [0, 0, 0], "white text on black ({inked} px)");
}

#[test]
fn dissolves_and_fades_mix_light() {
    let Some(g) = gpu() else { return eprintln!("skipped: no GPU") };
    let mut e = engine("dissolve");
    { let c = matte(&e, [1.0, 0.0, 0.0], 0, 4); put(&mut e, 0, c) };
    let blue = { let c = matte(&e, [0.0, 0.0, 1.0], 4, 4); put(&mut e, 0, c) };
    let dissolve = |before, after| Transition {
        id: EffectId::new(),
        plugin: intrinsic::plugin_ref(intrinsic::DISSOLVE),
        before: Time::from_seconds(before),
        after: Time::from_seconds(after),
        params: OrdMap::new(),
    };
    let snap = e.snapshot();
    e.execute(edit::set_transition(&snap, blue, Edge::Start, Some(dissolve(1, 1))).unwrap()).unwrap();
    // At the cut, half way: half red, half blue light → 0.5^(1/2.4) = 191.
    assert!(near(px(&shot(&e, &g, 4.0, false), 32, 18), [191, 0, 191]), "{:?}", px(&shot(&e, &g, 4.0, false), 32, 18));
    assert!(near(px(&shot(&e, &g, 2.0, false), 32, 18), [255, 0, 0]));
    assert!(near(px(&shot(&e, &g, 6.0, false), 32, 18), [0, 0, 255]));
    // A fade in from nothing on a clip after a gap, half way through.
    let green = { let c = matte(&e, [0.0, 1.0, 0.0], 10, 4); put(&mut e, 0, c) };
    let snap = e.snapshot();
    e.execute(edit::set_transition(&snap, green, Edge::Start, Some(dissolve(0, 2))).unwrap()).unwrap();
    // (Within 4: pure 709 green in an ACEScg working space carries red and
    // blue components; rounded to half floats through the transition's
    // three passes they leave ~3e-5 behind, which display encoding near
    // black shows as a few code values. Exact in a 709 working space.)
    let fade = px(&shot(&e, &g, 11.0, false), 32, 18);
    assert!(fade.iter().zip([0u8, 191, 0]).all(|(a, b)| a.abs_diff(b) <= 4), "{fade:?}");
}

#[test]
fn nested_and_multicam_clips_show_their_sequence() {
    let Some(g) = gpu() else { return eprintln!("skipped: no GPU") };
    let mut e = engine("nest");
    let red = { let c = matte(&e, [1.0, 0.0, 0.0], 0, 4); put(&mut e, 0, c) };
    let blue = { let c = matte(&e, [0.0, 0.0, 1.0], 0, 4); put(&mut e, 1, c) };
    // Nest both: V2's blue still covers V1's red inside the compound clip.
    let snap = e.snapshot();
    let seq = snap.active().unwrap().clone();
    let plugins = e.plugins().clone();
    let cmd = edit::nest(&snap, seq.id, &[red, blue], "Nest", |s, k, l| make_sequence_clip(&plugins, &seq.format, s, k, l)).unwrap();
    e.execute(cmd).unwrap();
    let snap = e.snapshot();
    let outer = snap.active().unwrap();
    assert_eq!(outer.tracks[1].clips.len(), 0, "the clips moved inside");
    let nest = outer.tracks[0].clips[0].clone();
    assert!(matches!(nest.source, ClipSource::Sequence { .. }));
    assert!(near(px(&shot(&e, &g, 1.0, false), 32, 18), [0, 0, 255]));
    // As a multicam clip: angle 0 is V1 of the nest (red), angle 1 V2 (blue).
    for (angle, want) in [(0, [255, 0, 0]), (1, [0, 0, 255])] {
        let snap = e.snapshot();
        e.execute(edit::set_angle(&snap, nest.id, Some(angle)).unwrap()).unwrap();
        assert!(near(px(&shot(&e, &g, 1.0, false), 32, 18), want), "angle {angle}");
    }
    // The compound clip's own Motion applies to the whole nest: half size.
    let snap = e.snapshot();
    let mut half = (**snap.find_clip(nest.id).unwrap().2).clone();
    let m = Arc::make_mut(&mut half.effects[0]);
    m.params.insert("scale".into(), Param::Constant(Value::Float(50.0)));
    e.execute(Command::SetClip { clip: Arc::new(half) }).unwrap();
    let img = shot(&e, &g, 1.0, false);
    assert!(near(px(&img, 32, 18), [0, 0, 255]) && px(&img, 2, 2) == [0, 0, 0], "a half-size nest over black");
    // And one undo step at a time takes it all back.
    while e.undo_label() != Some("Nest") {
        assert!(e.undo());
    }
    assert!(e.undo());
    assert_eq!(e.snapshot().active().unwrap().tracks[1].clips.len(), 1);
}

#[test]
fn proxies_stand_in_at_the_original_size() {
    let Some(g) = gpu() else { return eprintln!("skipped: no GPU") };
    let dir = tmp("proxy-src");
    let make = |name: &str, color: &str, size: &str| {
        let p = dir.join(name);
        let ok = std::process::Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", &format!("color=c={color}:size={size}:rate=25:duration=1"), "-c:v", "mpeg4", "-q:v", "2"])
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success());
        ok.then_some(p)
    };
    let (Some(orig), Some(proxy)) = (make("orig.mov", "red", "32x18"), make("proxy.mov", "blue", "16x8")) else {
        return eprintln!("skipped: no ffmpeg CLI");
    };
    let mut e = engine("proxy");
    let asset = e.import(orig.to_str().unwrap()).unwrap();
    e.attach_proxy(asset, proxy.to_str().unwrap()).unwrap();
    let snap = e.snapshot();
    let seq = snap.active().unwrap().clone();
    // The 32x18 original at 100% fills the middle of the 64x36 frame.
    let mut c = make_clip(e.plugins(), &seq.format, &snap.assets[&asset], TrackKind::Video, Time::from_seconds(1), None);
    Arc::make_mut(&mut c.effects[0]).params.insert("scale".into(), Param::Constant(Value::Float(100.0)));
    put(&mut e, 0, c);
    let (full, prox) = (shot(&e, &g, 0.5, false), shot(&e, &g, 0.5, true));
    assert!(px(&full, 32, 18)[0] > 200 && px(&prox, 32, 18)[2] > 200, "original red, proxy blue");
    // The proxy covers exactly the original's area (not half of it).
    for (x, y) in [(18, 11), (46, 25)] {
        assert!(px(&prox, x, y)[2] > 200, "proxy reaches ({x},{y})");
    }
    assert_eq!(px(&prox, 10, 5), [0, 0, 0], "and no further");
}

#[test]
fn export_renders_the_in_out_range() {
    let Some(gpu) = gpu() else { return eprintln!("skipped: no GPU") };
    let mut e = engine("inout");
    { let c = matte(&e, [1.0, 1.0, 1.0], 0, 10); put(&mut e, 0, c) };
    let seq = e.snapshot().active().unwrap().clone();
    let marks = Marks { in_point: Some(Time::from_seconds(2)), out_point: Some(Time::from_seconds(5)), markers: Default::default() };
    e.execute(Command::SetMarks { owner: MarksOwner::Sequence(seq.id), marks }).unwrap();
    let client = e.spawn(Arc::new(|| {}));
    let out = tmp("inout-out").join("range.mov");
    let job = client.export(gpu, ExportPreset::ProResMov, MediaRef(format!("file:{}", out.display())), None);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    while job.result().is_none() {
        assert!(std::time::Instant::now() < deadline, "export hung");
        std::thread::sleep(Duration::from_millis(20));
    }
    job.result().unwrap().unwrap();
    assert_eq!(job.total, 72, "3 s at 24 fps");
    use ve_ports::{MediaBackend, Resolved};
    let info = media_ffmpeg::Ffmpeg::new().probe(&Resolved { path: Some(out), guard: Box::new(()) }).unwrap();
    assert!((info.duration.as_seconds_f64() - 3.0).abs() < 0.1, "{}", info.duration.as_seconds_f64());
}
