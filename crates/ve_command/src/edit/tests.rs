use super::*;
use ve_time::TimeRange;

fn s(x: i64) -> Time {
    Time::from_seconds(x)
}

struct F {
    p: Project,
    seq: SequenceId,
    v1: TrackId,
    a1: TrackId,
    asset: AssetId,
}

fn fixture() -> F {
    let mut p = Project::new("t");
    let asset = Asset {
        id: AssetId::new(),
        name: "a".into(),
        media: MediaRef("file:a".into()),
        info: Some(MediaInfo { duration: s(100), video: None, audio: None }),
    };
    let (v1, a1) = (Track::new(TrackKind::Video, "V1"), Track::new(TrackKind::Audio, "A1"));
    let seq = Sequence {
        id: SequenceId::new(),
        name: "S".into(),
        format: SequenceFormat::default(),
        tracks: [v1.clone(), a1.clone()].into_iter().map(Arc::new).collect(),
    };
    let f = F { seq: seq.id, v1: v1.id, a1: a1.id, asset: asset.id, p: Project::new("t") };
    p.assets.insert(asset.id, Arc::new(asset));
    p.sequences.insert(seq.id, Arc::new(seq));
    F { p, ..f }
}

fn clip(f: &F, start: i64, src: i64, len: i64) -> Arc<Clip> {
    linked_clip(f, start, src, len, None)
}

fn linked_clip(f: &F, start: i64, src: i64, len: i64, link: Option<LinkId>) -> Arc<Clip> {
    Arc::new(Clip {
        id: ClipId::new(),
        name: "c".into(),
        source: ClipSource::Asset { asset: f.asset },
        source_range: TimeRange::new(s(src), s(len)),
        timeline_start: s(start),
        enabled: true,
        link,
        effects: Default::default(),
    })
}

/// Add `c` to V1 (`video`) or A1.
fn put(f: &mut F, video: bool, c: &Arc<Clip>) {
    let track = if video { f.v1 } else { f.a1 };
    f.p = Command::AddClip { sequence: f.seq, track, clip: c.clone() }.apply(&f.p).unwrap().project;
}

/// (start, source start, duration) of each clip on `track`, in seconds.
fn layout(p: &Project, seq: SequenceId, track: TrackId) -> Vec<(i64, i64, i64)> {
    let t = p.sequence(seq).unwrap().track(track).unwrap().1.clone();
    let sec = |t: Time| t.ticks() / ve_time::TICKS_PER_SECOND;
    t.clips.iter().map(|c| (sec(c.timeline_start), sec(c.source_range.start), sec(c.source_range.duration))).collect()
}

fn run(f: &F, cmd: Command) -> Project {
    let applied = cmd.apply(&f.p).unwrap();
    // Every tool must undo exactly.
    assert_eq!(applied.inverse.apply(&applied.project).unwrap().project, f.p, "undo restores");
    applied.project
}

#[test]
fn razor_splits_linked_clips_together() {
    let mut f = fixture();
    let l = LinkId::new();
    let (v, a) = (linked_clip(&f, 0, 10, 10, Some(l)), linked_clip(&f, 0, 10, 10, Some(l)));
    put(&mut f, true, &v);
    put(&mut f, false, &a);
    let p = run(&f, razor(&f.p, &[v.id], s(4), true).unwrap());
    assert_eq!(layout(&p, f.seq, f.v1), [(0, 10, 4), (4, 14, 6)]);
    assert_eq!(layout(&p, f.seq, f.a1), [(0, 10, 4), (4, 14, 6)]);
    // The right halves are linked to each other, not to the left halves.
    let seq = p.sequence(f.seq).unwrap();
    let (vr, ar) = (&seq.tracks[0].clips[1], &seq.tracks[1].clips[1]);
    assert_eq!(vr.link, ar.link);
    assert_ne!(vr.link, Some(l));
}

#[test]
fn ripple_delete_closes_the_gap() {
    let mut f = fixture();
    let (a, b, c) = (clip(&f, 0, 0, 3), clip(&f, 3, 0, 4), clip(&f, 9, 0, 2));
    for x in [&a, &b, &c] {
        put(&mut f, true, x);
    }
    let p = run(&f, ripple_delete(&f.p, &[b.id], true).unwrap());
    assert_eq!(layout(&p, f.seq, f.v1), [(0, 0, 3), (5, 0, 2)]);
}

#[test]
fn ripple_trim_pushes_and_pulls() {
    let mut f = fixture();
    let (a, b) = (clip(&f, 0, 0, 5), clip(&f, 5, 0, 5));
    put(&mut f, true, &a);
    put(&mut f, true, &b);
    let longer = run(&f, ripple_trim(&f.p, a.id, Edge::End, s(2), true).unwrap());
    assert_eq!(layout(&longer, f.seq, f.v1), [(0, 0, 7), (7, 0, 5)]);
    let shorter = run(&f, ripple_trim(&f.p, a.id, Edge::End, -s(2), true).unwrap());
    assert_eq!(layout(&shorter, f.seq, f.v1), [(0, 0, 3), (3, 0, 5)]);
    // Start edge: the clip stays put and shows later frames; b follows.
    let head = run(&f, ripple_trim(&f.p, a.id, Edge::Start, s(1), true).unwrap());
    assert_eq!(layout(&head, f.seq, f.v1), [(0, 1, 4), (4, 0, 5)]);
}

#[test]
fn roll_keeps_the_total() {
    let mut f = fixture();
    let (a, b) = (clip(&f, 0, 0, 5), clip(&f, 5, 10, 5));
    put(&mut f, true, &a);
    put(&mut f, true, &b);
    let p = run(&f, roll(&f.p, a.id, b.id, s(2)).unwrap());
    assert_eq!(layout(&p, f.seq, f.v1), [(0, 0, 7), (7, 12, 3)]);
    let p = run(&f, roll(&f.p, a.id, b.id, -s(2)).unwrap());
    assert_eq!(layout(&p, f.seq, f.v1), [(0, 0, 3), (3, 8, 7)]);
}

#[test]
fn slip_changes_frames_not_position_and_is_bounded() {
    let mut f = fixture();
    let a = clip(&f, 2, 10, 5);
    put(&mut f, true, &a);
    let p = run(&f, slip(&f.p, a.id, s(3), true).unwrap());
    assert_eq!(layout(&p, f.seq, f.v1), [(2, 13, 5)]);
    assert_eq!(slip(&f.p, a.id, -s(11), true).unwrap_err(), CommandError::BeyondSource);
}

#[test]
fn insert_splits_and_pushes() {
    let mut f = fixture();
    let (a, b) = (clip(&f, 0, 0, 10), clip(&f, 10, 0, 5));
    put(&mut f, true, &a);
    put(&mut f, true, &b);
    let new = clip(&f, 0, 50, 3);
    let p = run(&f, insert(&f.p, f.seq, s(4), &[(f.v1, new)]).unwrap());
    assert_eq!(layout(&p, f.seq, f.v1), [(0, 0, 4), (4, 50, 3), (7, 4, 6), (13, 0, 5)]);
}

#[test]
fn overwrite_replaces_the_middle() {
    let mut f = fixture();
    let a = clip(&f, 0, 0, 10);
    put(&mut f, true, &a);
    let new = clip(&f, 0, 50, 3);
    let p = run(&f, overwrite(&f.p, f.seq, s(4), &[(f.v1, new)]).unwrap());
    assert_eq!(layout(&p, f.seq, f.v1), [(0, 0, 4), (4, 50, 3), (7, 7, 3)]);
}

#[test]
fn linked_move_carries_partners() {
    let mut f = fixture();
    let l = LinkId::new();
    let (v, a) = (linked_clip(&f, 0, 0, 4, Some(l)), linked_clip(&f, 0, 0, 4, Some(l)));
    put(&mut f, true, &v);
    put(&mut f, false, &a);
    let p = run(&f, move_clip(&f.p, v.id, f.v1, s(6), true).unwrap());
    assert_eq!(layout(&p, f.seq, f.v1), [(6, 0, 4)]);
    assert_eq!(layout(&p, f.seq, f.a1), [(6, 0, 4)]);
    let solo = run(&f, move_clip(&f.p, v.id, f.v1, s(6), false).unwrap());
    assert_eq!(layout(&solo, f.seq, f.a1), [(0, 0, 4)]);
}

#[test]
fn snap_points_skip_dragged_clips() {
    let mut f = fixture();
    let (a, b) = (clip(&f, 0, 0, 3), clip(&f, 5, 0, 2));
    put(&mut f, true, &a);
    put(&mut f, true, &b);
    let pts = snap_points(f.p.sequence(f.seq).unwrap(), &[b.id], s(9));
    assert_eq!(pts, [s(0), s(3), s(9)]);
}
