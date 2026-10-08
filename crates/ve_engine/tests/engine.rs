use std::sync::Arc;
use platform_headless::NoMedia;
use ve_engine::*;
use ve_model::*;
use ve_time::{Time, TimeRange};

fn engine(dir: &std::path::Path) -> Engine {
    let mut e = Engine::new(platform_headless::platform(dir, Arc::new(NoMedia)));
    e.new_project("Test");
    e
}

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn add_generator_clip(e: &mut Engine, start: i64, len: i64) -> ClipId {
    let snap = e.snapshot();
    let seq = snap.active().unwrap();
    let clip = Clip {
        id: ClipId::new(),
        name: "Bars".into(),
        source: ClipSource::Generator {
            plugin: PluginRef { api: PluginApi::Builtin, id: "bars".into(), major_version: 1 },
        },
        source_range: TimeRange::new(Time::ZERO, Time::from_seconds(len)),
        timeline_start: Time::from_seconds(start),
        enabled: true,
        link: None,
        effects: Default::default(),
    };
    let id = clip.id;
    e.execute(Command::AddClip { sequence: seq.id, track: seq.tracks[0].id, clip: Arc::new(clip) }).unwrap();
    id
}

#[test]
fn edit_undo_redo_through_the_api() {
    let mut e = engine(&tmp("edit"));
    let c = add_generator_clip(&mut e, 0, 5);
    assert_eq!(e.snapshot().active().unwrap().duration(), Time::from_seconds(5));
    assert_eq!(e.undo_label(), Some("Add Clip"));
    let held = e.snapshot();
    assert!(e.undo());
    assert!(e.snapshot().find_clip(c).is_none());
    assert!(held.find_clip(c).is_some(), "a snapshot taken earlier is unaffected");
    assert!(e.redo());
    assert!(e.snapshot().find_clip(c).is_some());
    assert!(e.drain_events().contains(&Event::ProjectChanged));
}

#[test]
fn save_and_open_round_trip() {
    let dir = tmp("save");
    let mut e = engine(&dir);
    add_generator_clip(&mut e, 2, 3);
    assert!(e.is_dirty());
    let file = MediaRef(format!("file:{}", dir.join("p.veproj").display()));
    e.save_as(file.clone()).unwrap();
    assert!(!e.is_dirty());
    let saved = e.snapshot();

    let mut other = engine(&dir);
    other.open(file).unwrap();
    assert_eq!(*other.snapshot(), *saved);
}

#[test]
fn import_without_a_media_backend_fails_cleanly() {
    let dir = tmp("import");
    std::fs::write(dir.join("x.mov"), b"not really").unwrap();
    let mut e = engine(&dir);
    let before = e.snapshot();
    assert!(e.import(dir.join("x.mov").to_str().unwrap()).is_err());
    assert_eq!(*e.snapshot(), *before);
}

#[test]
fn playback_and_frame_plan() {
    let mut e = engine(&tmp("play"));
    let c = add_generator_clip(&mut e, 1, 3);
    e.seek(Time::from_seconds(2));
    let plan = e.frame_plan(Quality::FULL).unwrap();
    assert_eq!(plan.layers.len(), 1);
    assert_eq!(plan.layers[0].clip, c);
    assert_eq!(plan.layers[0].clip_time, Time::from_seconds(1));
    e.play(1.0);
    assert!(e.is_playing());
    e.stop();
    assert!(!e.is_playing());
}

#[test]
fn threaded_client_round_trip() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = tmp("client");
    let wakes = Arc::new(AtomicUsize::new(0));
    let w = wakes.clone();
    let client = engine(&dir).spawn(Arc::new(move || {
        w.fetch_add(1, Ordering::SeqCst);
    }));
    let seq = client.snapshot().active().unwrap().clone();
    let clip = Clip {
        id: ClipId::new(),
        name: "c".into(),
        source: ClipSource::Generator { plugin: PluginRef { api: PluginApi::Builtin, id: "bars".into(), major_version: 1 } },
        source_range: TimeRange::new(Time::ZERO, Time::from_seconds(2)),
        timeline_start: Time::ZERO,
        enabled: true,
        link: None,
        effects: Default::default(),
    };
    client.execute(Command::AddClip { sequence: seq.id, track: seq.tracks[0].id, clip: Arc::new(clip) });
    // Seek applies locally at once.
    client.seek(Time::from_seconds(1));
    assert_eq!(client.playhead(), Time::from_seconds(1));
    // The edit arrives once the engine thread has run it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while client.published().undo_label.is_none() {
        assert!(std::time::Instant::now() < deadline, "engine never published");
        std::thread::yield_now();
    }
    assert_eq!(client.published().undo_label.as_deref(), Some("Add Clip"));
    assert!(wakes.load(Ordering::SeqCst) > 0);
    // Intrinsic effects are registered.
    assert!(client.plugins().find_id(PluginApi::Builtin, "ve.motion").is_some());
}

#[test]
fn make_clip_attaches_intrinsics() {
    let e = engine(&tmp("intrinsics"));
    let asset = Asset {
        id: AssetId::new(),
        name: "a.mov".into(),
        media: MediaRef("file:a.mov".into()),
        info: Some(MediaInfo {
            duration: Time::from_seconds(3),
            video: Some(VideoStreamInfo { width: 1280, height: 720, rate: ve_time::Rate::FPS_25, codec: "h264".into() }),
            audio: None,
        }),
    };
    let fmt = SequenceFormat::default();
    let c = make_clip(e.plugins(), &fmt, &asset, TrackKind::Video, Time::from_seconds(3), None);
    assert_eq!(c.effects.len(), 2);
    let motion = &c.effects[0];
    assert_eq!(motion.params["position"], Param::Constant(Value::Vec2([960.0, 540.0])));
    assert_eq!(motion.params["anchor"], Param::Constant(Value::Vec2([640.0, 360.0])));
}
