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
        retime: Default::default(),
        transition_in: None,
        transition_out: None,
        channels: Vec::new(),
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
        retime: Default::default(),
        transition_in: None,
        transition_out: None,
        channels: Vec::new(),
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
            video: Some(VideoStreamInfo::new(1280, 720, ve_time::Rate::FPS_25, "h264")),
            audio: Vec::new(),
        }),
        variants: Vec::new(),
        marks: Default::default(),
    };
    let fmt = SequenceFormat::default();
    let c = make_clip(e.plugins(), &fmt, &asset, TrackKind::Video, Time::from_seconds(3), None);
    assert_eq!(c.effects.len(), 2);
    let motion = &c.effects[0];
    assert_eq!(motion.params["position"], Param::Constant(Value::Vec2([960.0, 540.0])));
    assert_eq!(motion.params["anchor"], Param::Constant(Value::Vec2([640.0, 360.0])));
}

#[test]
fn save_replaces_the_file_whole() {
    let dir = tmp("atomic");
    let mut e = engine(&dir);
    let path = dir.join("p.veproj");
    let file = MediaRef(format!("file:{}", path.display()));
    add_generator_clip(&mut e, 0, 1);
    e.save_as(file.clone()).unwrap();
    add_generator_clip(&mut e, 2, 1);
    e.save_as(file.clone()).unwrap();
    // No temporary files left beside it, and it reads back as the latest.
    let names: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
    assert_eq!(names.iter().filter(|n| n.to_string_lossy().contains(".tmp")).count(), 0, "{names:?}");
    let mut other = engine(&dir);
    other.open(file).unwrap();
    assert_eq!(*other.snapshot(), *e.snapshot());
    // A save that cannot happen leaves the old file as it was.
    let before = std::fs::read(&path).unwrap();
    let blocked = MediaRef(format!("file:{}", path.join("inside-a-file.veproj").display()));
    assert!(e.save_as(blocked).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn autosave_writes_unsaved_changes_only() {
    let dir = tmp("autosave");
    let mut e = engine(&dir);
    assert!(!e.autosave().unwrap(), "nothing to autosave in a fresh project");
    add_generator_clip(&mut e, 0, 2);
    assert!(e.autosave().unwrap());
    assert!(!e.autosave().unwrap(), "unchanged since the last autosave");
    let auto = e.autosave_ref().unwrap();
    let path = platform_headless::FileStorage::path_of(&auto).unwrap();
    assert!(path.starts_with(dir.join("data/Autosave")), "{}", path.display());
    // The autosave is a project file like any other.
    let mut other = engine(&dir);
    other.open(auto).unwrap();
    assert_eq!(*other.snapshot(), *e.snapshot());
    // Saving for real retires it.
    e.save_as(MediaRef(format!("file:{}", dir.join("p.veproj").display()))).unwrap();
    assert!(!path.exists());
}

/// Dropping a file with more audio streams than the sequence has audio
/// tracks: tracks are added, and every stream gets a linked clip.
#[test]
fn every_audio_stream_gets_a_track_and_a_clip() {
    let mut e = engine(&tmp("streams"));
    let stream = |layout: &str, channels| AudioStreamInfo { sample_rate: 48_000, channels, codec: "pcm_s24le".into(), layout: layout.into() };
    let asset = Asset {
        id: AssetId::new(),
        name: "cam.mxf".into(),
        media: MediaRef("file:cam.mxf".into()),
        info: Some(MediaInfo {
            duration: Time::from_seconds(4),
            video: Some(VideoStreamInfo::new(1920, 1080, ve_time::Rate::FPS_25, "h264")),
            audio: vec![stream("mono", 1), stream("mono", 1), stream("mono", 1), stream("stereo", 2)],
        }),
        variants: Vec::new(),
        marks: Default::default(),
    };
    e.execute(Command::AddAsset { asset: Arc::new(asset.clone()) }).unwrap();
    let before = e.snapshot();
    let seq = before.active().unwrap().clone(); // V1 V2 A1 A2
    let (adds, items) = clips_for_asset(e.plugins(), &seq, &asset, Time::from_seconds(4), None, None);
    assert_eq!((adds.len(), items.len()), (2, 5), "A3 and A4 added; picture + 4 streams");
    let with_tracks = Command::Batch { label: String::new(), commands: adds.clone() }.apply(&before).unwrap().project;
    let edit = edit::overwrite(&with_tracks, seq.id, Time::ZERO, &items).unwrap();
    e.execute(Command::Batch { label: "Overwrite".into(), commands: adds.into_iter().chain([edit]).collect() }).unwrap();

    let snap = e.snapshot();
    let seq = snap.active().unwrap();
    let audio: Vec<_> = seq.tracks.iter().filter(|t| t.kind == TrackKind::Audio).collect();
    assert_eq!(audio.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["A1", "A2", "A3", "A4"]);
    let streams: Vec<u32> = audio
        .iter()
        .map(|t| match &t.clips[0].source {
            ClipSource::Asset { audio_stream, .. } => *audio_stream,
            _ => panic!(),
        })
        .collect();
    assert_eq!(streams, [0, 1, 2, 3]);
    let link = seq.tracks[0].clips[0].link;
    assert!(link.is_some() && audio.iter().all(|t| t.clips[0].link == link), "all linked to the picture");
    // One undo step takes it all back, tracks included.
    assert!(e.undo());
    assert_eq!(*e.snapshot(), *before);
}

/// A clip cannot name an audio stream its media does not have.
#[test]
fn clips_must_name_an_existing_stream() {
    let mut e = engine(&tmp("badstream"));
    let asset = Asset {
        id: AssetId::new(),
        name: "a.wav".into(),
        media: MediaRef("file:a.wav".into()),
        info: Some(MediaInfo { duration: Time::from_seconds(1), video: None, audio: vec![AudioStreamInfo { sample_rate: 48_000, channels: 2, codec: "pcm".into(), layout: "stereo".into() }] }),
        variants: Vec::new(),
        marks: Default::default(),
    };
    e.execute(Command::AddAsset { asset: Arc::new(asset.clone()) }).unwrap();
    let snap = e.snapshot();
    let seq = snap.active().unwrap();
    let mut clip = make_clip(e.plugins(), &seq.format, &asset, TrackKind::Audio, Time::from_seconds(1), None);
    clip.source = ClipSource::Asset { asset: asset.id, audio_stream: 1 };
    let err = e.execute(Command::AddClip { sequence: seq.id, track: seq.tracks[2].id, clip: Arc::new(clip) }).unwrap_err();
    assert!(matches!(err, CommandError::Invalid(ModelError::MissingAudioStream(_))), "{err:?}");
}

/// Every schema-3 feature survives a save and an open unchanged.
#[test]
fn new_features_round_trip_through_the_file() {
    let dir = tmp("schema3");
    let mut e = engine(&dir);
    let snap = e.snapshot();
    let seq = snap.active().unwrap().clone();
    // A nested sequence holding a sped-up, reversed generator with a fade.
    let mut inner_clip = make_generator_clip(e.plugins(), &seq.format, &intrinsic::plugin_ref(intrinsic::BARS), Time::from_seconds(4)).unwrap();
    inner_clip.retime = Retime::Remap(vec![RemapKey { time: Time::ZERO, offset: Time::from_seconds(2) }, RemapKey { time: Time::from_seconds(4), offset: Time::ZERO }]);
    inner_clip.transition_in = Some(Arc::new(Transition {
        id: EffectId::new(),
        plugin: intrinsic::plugin_ref(intrinsic::DIP_TO_BLACK),
        before: Time::ZERO,
        after: Time::from_seconds(1),
        params: OrdMap::new(),
    }));
    let mut v1 = Track::new(TrackKind::Video, "V1");
    v1.clips.push_back(Arc::new(inner_clip));
    let mut a1 = Track::new(TrackKind::Audio, "A1");
    a1.layout = ChannelLayout::Mono;
    let inner = Sequence::new("Nest", seq.format.clone(), [v1, a1]);
    e.execute(Command::AddSequence { sequence: Arc::new(inner.clone()) }).unwrap();
    let mut outer = make_sequence_clip(e.plugins(), &seq.format, &inner, TrackKind::Video, None);
    outer.source = ClipSource::Sequence { sequence: inner.id, angle: Some(0) };
    outer.retime = Retime::Speed(Ratio::new(24000, 25025));
    e.execute(Command::AddClip { sequence: seq.id, track: seq.tracks[0].id, clip: Arc::new(outer) }).unwrap();
    // Marks, a proxy, a track layout.
    let marker = Marker {
        id: MarkerId::new(),
        time: Time::from_seconds(1),
        duration: Time::ZERO,
        name: "Chapter 1".into(),
        comment: String::new(),
        color: MarkerColor::Blue,
        kind: MarkerKind::Chapter,
    };
    let marks = Marks { in_point: Some(Time::from_seconds(1)), out_point: None, markers: [marker].into_iter().collect() };
    e.execute(Command::SetMarks { owner: MarksOwner::Sequence(seq.id), marks }).unwrap();
    let mut asset = Asset::new("a.mov", MediaRef("file:a.mov".into()), None);
    asset.variants.push(MediaVariant { kind: VariantKind::Proxy, media: MediaRef("file:a_proxy.mov".into()), width: 960, height: 540 });
    e.execute(Command::AddAsset { asset: Arc::new(asset) }).unwrap();
    e.execute(Command::SetTrackLayout { sequence: seq.id, track: seq.tracks[3].id, layout: ChannelLayout::Mono }).unwrap();

    let file = MediaRef(format!("file:{}", dir.join("p.veproj").display()));
    e.save_as(file.clone()).unwrap();
    let mut other = engine(&dir);
    other.open(file).unwrap();
    assert_eq!(*other.snapshot(), *e.snapshot());
}

#[test]
fn a_sequence_cannot_contain_itself() {
    let mut e = engine(&tmp("cycle"));
    let snap = e.snapshot();
    let seq = snap.active().unwrap().clone();
    let inner = Sequence::new("Inner", seq.format.clone(), [Track::new(TrackKind::Video, "V1")]);
    e.execute(Command::AddSequence { sequence: Arc::new(inner.clone()) }).unwrap();
    // Outer holds Inner…
    let c = make_sequence_clip(e.plugins(), &seq.format, &inner, TrackKind::Video, None);
    e.execute(Command::AddClip { sequence: seq.id, track: seq.tracks[0].id, clip: Arc::new(c) }).unwrap();
    // …so Inner may not hold Outer.
    let back = make_sequence_clip(e.plugins(), &seq.format, &seq, TrackKind::Video, None);
    let err = e.execute(Command::AddClip { sequence: inner.id, track: inner.tracks[0].id, clip: Arc::new(back) }).unwrap_err();
    assert!(matches!(err, CommandError::Invalid(ModelError::NestingCycle(_))), "{err:?}");
}

/// The Source monitor: an asset plays as a sequence of its own, with its
/// own playhead; switching monitors keeps each one's position.
#[test]
fn the_source_monitor_has_its_own_playhead() {
    let mut e = engine(&tmp("source-monitor"));
    let stream = AudioStreamInfo { sample_rate: 48_000, channels: 2, codec: "pcm".into(), layout: "stereo".into() };
    let asset = Asset {
        id: AssetId::new(),
        name: "cam.mov".into(),
        media: MediaRef("file:cam.mov".into()),
        info: Some(MediaInfo {
            duration: Time::from_seconds(10),
            video: Some(VideoStreamInfo::new(3840, 2160, ve_time::Rate::FPS_25, "h264")),
            audio: vec![stream.clone(), stream],
        }),
        variants: Vec::new(),
        marks: Marks { in_point: Some(Time::from_seconds(2)), ..Default::default() },
    };
    e.execute(Command::AddAsset { asset: Arc::new(asset.clone()) }).unwrap();
    add_generator_clip(&mut e, 0, 8);
    e.seek(Time::from_seconds(1)); // the program playhead

    e.set_source(asset.id).unwrap();
    assert_eq!(e.viewer(), Viewer::Source);
    assert_eq!(e.playhead(), Time::from_seconds(2), "opens at its in point");
    let s = e.source().unwrap();
    assert_eq!(s.duration(), Time::from_seconds(10));
    assert_eq!((s.sequence.format.width, s.sequence.format.rate), (3840, ve_time::Rate::FPS_25), "the asset's own format");
    let audio = s.sequence.tracks.iter().filter(|t| t.kind == TrackKind::Audio && !t.clips.is_empty()).count();
    assert_eq!(audio, 2, "every audio stream plays");
    assert!(!e.snapshot().sequences.contains_key(&s.sequence.id), "not part of the project");
    assert_eq!(e.transport().end, Time::from_seconds(10));

    e.seek(Time::from_seconds(5));
    let (active, parked) = (e.transport().clone(), e.parked().clone());
    // Back to the program: its playhead is where it was.
    e.adopt_viewer(Viewer::Program, parked, active);
    assert_eq!(e.playhead(), Time::from_seconds(1));
    assert_eq!(e.parked().position_at(Clocks { audio: None, monotonic: ve_ports::clock_now() }), Time::from_seconds(5));

    // Removing the asset empties the Source monitor.
    e.execute(Command::RemoveAsset { asset: asset.id }).unwrap();
    assert!(e.source().is_none());
    assert_eq!(e.viewer(), Viewer::Program);
}
