use super::*;
use ve_time::{Time, TimeRange};

fn s(x: i64) -> Time {
    Time::from_seconds(x)
}

struct Fixture {
    p: Project,
    seq: SequenceId,
    v1: TrackId,
    v2: TrackId,
    a1: TrackId,
    asset: AssetId,
}

fn fixture() -> Fixture {
    let mut p = Project::new("t");
    let asset = Asset {
        id: AssetId::new(),
        name: "a.mp4".into(),
        media: MediaRef("file:a.mp4".into()),
        info: Some(MediaInfo { duration: s(20), video: None, audio: Vec::new() }),
        variants: Vec::new(),
        marks: Default::default(),
    };
    let (v1, v2, a1) =
        (Track::new(TrackKind::Video, "V1"), Track::new(TrackKind::Video, "V2"), Track::new(TrackKind::Audio, "A1"));
    let seq = Sequence {
        id: SequenceId::new(),
        name: "S".into(),
        format: SequenceFormat::default(),
        tracks: [v1.clone(), v2.clone(), a1.clone()].into_iter().map(Arc::new).collect(),
        marks: Default::default(),
    };
    let f = Fixture { seq: seq.id, v1: v1.id, v2: v2.id, a1: a1.id, asset: asset.id, p: Project::new("t") };
    p.assets.insert(asset.id, Arc::new(asset));
    p.sequences.insert(seq.id, Arc::new(seq));
    p.active_sequence = Some(f.seq);
    Fixture { p, ..f }
}

fn clip(f: &Fixture, start: i64, src: i64, len: i64) -> Arc<Clip> {
    Arc::new(Clip {
        id: ClipId::new(),
        name: "c".into(),
        source: ClipSource::Asset { asset: f.asset, audio_stream: 0 },
        source_range: TimeRange::new(s(src), s(len)),
        timeline_start: s(start),
        enabled: true,
        link: None,
        effects: Default::default(),
        retime: Default::default(),
        transition_in: None,
        transition_out: None,
        channels: Vec::new(),
    })
}

fn add(f: &Fixture, p: &Project, track: TrackId, c: &Arc<Clip>) -> Result<Project, CommandError> {
    Command::AddClip { sequence: f.seq, track, clip: c.clone() }.apply(p).map(|a| a.project)
}

#[test]
fn add_keeps_track_sorted_and_rejects_overlap() {
    let f = fixture();
    let (a, b) = (clip(&f, 10, 0, 5), clip(&f, 0, 0, 5));
    let p = add(&f, &f.p, f.v1, &a).unwrap();
    let p = add(&f, &p, f.v1, &b).unwrap();
    let t = &p.sequences[&f.seq].tracks[0];
    assert_eq!(t.clips[0].id, b.id);
    assert_eq!(add(&f, &p, f.v1, &clip(&f, 4, 0, 2)).unwrap_err(), CommandError::Overlap);
}

#[test]
fn move_between_tracks_and_undo() {
    let f = fixture();
    let c = clip(&f, 0, 0, 5);
    let mut h = History::default();
    let p0 = add(&f, &f.p, f.v1, &c).unwrap();
    let p1 = h.execute(&p0, Command::MoveClip { clip: c.id, track: f.v2, start: s(3) }).unwrap();
    let (seq, ti, moved) = p1.find_clip(c.id).unwrap();
    assert_eq!((seq.tracks[ti].id, moved.timeline_start), (f.v2, s(3)));
    let back = h.undo(&p1).unwrap().unwrap();
    assert_eq!(back, p0);
    assert_eq!(h.redo(&back).unwrap().unwrap(), p1);
}

#[test]
fn moving_onto_an_audio_track_breaks_nothing_else() {
    // Kind checks belong to the edit tools; the command only keeps invariants.
    let f = fixture();
    let c = clip(&f, 0, 0, 5);
    let p = add(&f, &f.p, f.v1, &c).unwrap();
    assert!(Command::MoveClip { clip: c.id, track: f.a1, start: s(0) }.apply(&p).is_ok());
}

#[test]
fn trim_start_keeps_frames_in_place() {
    let f = fixture();
    let c = clip(&f, 10, 2, 5); // shows source 2..7 at 10..15
    let p = add(&f, &f.p, f.v1, &c).unwrap();
    let p = Command::TrimClip { clip: c.id, edge: Edge::Start, delta: s(1) }.apply(&p).unwrap().project;
    let t = p.find_clip(c.id).unwrap().2;
    assert_eq!(t.timeline_range(), TimeRange::new(s(11), s(4)));
    assert_eq!(t.source_time(s(11)), s(3), "the frame at 11 s is unchanged");
}

#[test]
fn trim_is_limited_by_media_and_zero() {
    let f = fixture();
    let c = clip(&f, 10, 2, 5);
    let p = add(&f, &f.p, f.v1, &c).unwrap();
    let trim = |edge, d| Command::TrimClip { clip: c.id, edge, delta: d }.apply(&p).err();
    assert_eq!(trim(Edge::Start, -s(3)), Some(CommandError::BeyondSource), "source starts at 0");
    assert_eq!(trim(Edge::End, s(14)), Some(CommandError::BeyondSource), "source is 20 s");
    assert_eq!(trim(Edge::End, -s(5)), Some(CommandError::BadRange));
}

#[test]
fn batch_is_all_or_nothing() {
    let f = fixture();
    let (a, b) = (clip(&f, 0, 0, 5), clip(&f, 5, 0, 5));
    let p = add(&f, &f.p, f.v1, &a).unwrap();
    let cmd = Command::Batch {
        label: "x".into(),
        commands: vec![
            Command::AddClip { sequence: f.seq, track: f.v1, clip: b.clone() },
            Command::MoveClip { clip: a.id, track: f.v1, start: s(6) }, // overlaps b
        ],
    };
    assert_eq!(cmd.apply(&p).unwrap_err(), CommandError::Overlap);
}

#[test]
fn remove_asset_in_use_is_refused() {
    let f = fixture();
    let c = clip(&f, 0, 0, 5);
    let p = add(&f, &f.p, f.v1, &c).unwrap();
    assert_eq!(Command::RemoveAsset { asset: f.asset }.apply(&p).unwrap_err(), CommandError::InUse(c.id));
}

#[test]
fn commands_serialize() {
    let f = fixture();
    let cmd = Command::MoveClip { clip: ClipId::new(), track: f.v1, start: s(1) };
    let back: Command = serde_json::from_str(&serde_json::to_string(&cmd).unwrap()).unwrap();
    assert_eq!(cmd, back);
}

#[test]
fn dirty_tracking() {
    let f = fixture();
    let mut h = History::default();
    h.mark_saved();
    assert!(!h.is_dirty());
    let p = h.execute(&f.p, Command::Rename { name: "x".into() }).unwrap();
    assert!(h.is_dirty());
    h.undo(&p).unwrap().unwrap();
    assert!(!h.is_dirty());
}
