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
        info: Some(MediaInfo { duration: s(100), video: None, audio: Vec::new() }),
        variants: Vec::new(),
        marks: Default::default(),
    };
    let (v1, a1) = (Track::new(TrackKind::Video, "V1"), Track::new(TrackKind::Audio, "A1"));
    let seq = Sequence {
        id: SequenceId::new(),
        name: "S".into(),
        format: SequenceFormat::default(),
        tracks: [v1.clone(), a1.clone()].into_iter().map(Arc::new).collect(),
        marks: Default::default(),
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
        source: ClipSource::Asset { asset: f.asset, audio_stream: 0 },
        source_range: TimeRange::new(s(src), s(len)),
        timeline_start: s(start),
        enabled: true,
        link,
        effects: Default::default(),
        retime: Default::default(),
        transition_in: None,
        transition_out: None,
        channels: Vec::new(),
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

/// A clip of source [10, 30) at 0 with an opacity ramp keyed 0→100 over
/// clip time [2 s, 6 s) — source [12, 16).
fn keyed(f: &F, start: i64) -> Arc<Clip> {
    let mut c = (*clip(f, start, 10, 20)).clone();
    let key = |t: i64, v: f64| Keyframe { time: s(t), value: Value::Float(v), interp: Interp::Linear };
    let mut params = OrdMap::new();
    params.insert("opacity".to_string(), Param::Animated(vec![key(2, 0.0), key(6, 100.0)]));
    let plugin = PluginRef { api: PluginApi::Builtin, id: "ve.opacity".into(), major_version: 1 };
    c.effects.push_back(Arc::new(Effect { id: EffectId::new(), plugin, enabled: true, params }));
    Arc::new(c)
}

/// Opacity of `id` at source time `src` (seconds).
fn opacity_at_source(p: &Project, id: ClipId, src: i64) -> Value {
    let (_, _, c) = p.find_clip(id).unwrap();
    c.effects[0].params["opacity"].value_at(s(src) - c.source_range.start)
}

#[test]
fn keyframes_stay_on_their_source_frames_through_head_trims() {
    let mut f = fixture();
    let c = keyed(&f, 0);
    put(&mut f, true, &c);
    let before: Vec<Value> = (12..=16).map(|src| opacity_at_source(&f.p, c.id, src)).collect();
    // Plain head trim, ripple head trim, and a roll that moves the right
    // clip's head: every source frame keeps its value.
    let trims = [
        Command::TrimClip { clip: c.id, edge: Edge::Start, delta: s(1) },
        ripple_trim(&f.p, c.id, Edge::Start, s(1), false).unwrap(),
    ];
    for cmd in trims {
        let p = run(&f, cmd);
        let after: Vec<Value> = (12..=16).map(|src| opacity_at_source(&p, c.id, src)).collect();
        assert_eq!(after, before);
    }
    let mut g = fixture();
    let left = clip(&g, 0, 0, 5);
    put(&mut g, true, &left);
    let right = keyed(&g, 5);
    put(&mut g, true, &right);
    let before: Vec<Value> = (12..=16).map(|src| opacity_at_source(&g.p, right.id, src)).collect();
    let p = run(&g, roll(&g.p, left.id, right.id, s(1)).unwrap());
    let after: Vec<Value> = (12..=16).map(|src| opacity_at_source(&p, right.id, src)).collect();
    assert_eq!(after, before);
}

#[test]
fn razor_and_head_trim_agree() {
    // Razor at clip time 3 s, or trim the head by 3 s: same keys either way.
    let mut f = fixture();
    let c = keyed(&f, 0);
    put(&mut f, true, &c);
    let cut = run(&f, razor(&f.p, &[c.id], s(3), false).unwrap());
    let right = cut.sequence(f.seq).unwrap().track(f.v1).unwrap().1.clips[1].clone();
    let trimmed = run(&f, Command::TrimClip { clip: c.id, edge: Edge::Start, delta: s(3) });
    let (_, _, t) = trimmed.find_clip(c.id).unwrap();
    assert_eq!(right.effects[0].params, t.effects[0].params);
}

fn get(p: &Project, id: ClipId) -> Arc<Clip> {
    p.find_clip(id).unwrap().2.clone()
}

#[test]
fn double_speed_halves_the_clip_and_ripples() {
    let mut f = fixture();
    let a = clip(&f, 0, 10, 8); // media 10..18
    let b = clip(&f, 8, 40, 4);
    put(&mut f, true, &a);
    put(&mut f, true, &b);
    let p = run(&f, set_speed(&f.p, a.id, Ratio::new(2, 1), true).unwrap());
    let a2 = get(&p, a.id);
    assert_eq!((a2.timeline_range().duration, a2.media_extent()), (s(4), ve_time::TimeRange::new(s(10), s(8))), "same media, half the time");
    assert_eq!(a2.source_time(s(3)), s(16));
    assert_eq!(get(&p, b.id).timeline_start, s(4), "the next clip closes up");
}

#[test]
fn cuts_and_head_trims_keep_frames_on_their_media_at_any_speed() {
    let mut f = fixture();
    let a = clip(&f, 0, 10, 8);
    put(&mut f, true, &a);
    f.p = set_speed(&f.p, a.id, Ratio::new(2, 1), false).unwrap().apply(&f.p).unwrap().project; // 4 s on the timeline
    // Razor at 1 s: both sides show media 12 s at the cut.
    let p = run(&f, razor(&f.p, &[a.id], s(1), false).unwrap());
    let clips = p.sequence(f.seq).unwrap().track(f.v1).unwrap().1.clips.clone();
    assert!((clips[0].source_time(s(1) - Time(1)) - s(12)).ticks().abs() <= 2, "continuous across the cut");
    assert_eq!(clips[1].source_time(s(1)), s(12));
    // A head trim of 1 s skips 2 s of media.
    let p = run(&f, Command::TrimClip { clip: a.id, edge: Edge::Start, delta: s(1) });
    assert_eq!(get(&p, a.id).source_time(s(1)), s(12));
    // Extending past the media is refused: at 2x each timeline second is two
    // of media.
    let mut g = fixture();
    let c = clip(&g, 0, 90, 8); // media 90..98 of 100
    put(&mut g, true, &c);
    g.p = set_speed(&g.p, c.id, Ratio::new(2, 1), false).unwrap().apply(&g.p).unwrap().project;
    assert!(Command::TrimClip { clip: c.id, edge: Edge::End, delta: s(1) }.apply(&g.p).is_ok(), "to media 100, the end");
    assert_eq!(Command::TrimClip { clip: c.id, edge: Edge::End, delta: s(2) }.apply(&g.p).unwrap_err(), CommandError::BeyondSource);
}

#[test]
fn reverse_plays_the_same_span_backwards() {
    let mut f = fixture();
    let a = clip(&f, 0, 10, 8);
    put(&mut f, true, &a);
    let p = run(&f, set_speed(&f.p, a.id, Ratio::new(-1, 1), false).unwrap());
    let r = get(&p, a.id);
    assert!(r.source_time(s(1)) > r.source_time(s(2)), "media runs down");
    assert_eq!(r.source_time(Time::ZERO), s(18) - Time(1), "from the last frame");
    let e = r.media_extent();
    assert!(e.start >= s(10) && e.end() <= s(18));
}

#[test]
fn marks_media_and_layouts_change_and_undo() {
    let f = fixture();
    let mut m = Marks { in_point: Some(s(2)), out_point: Some(s(5)), ..Default::default() };
    run(&f, Command::SetMarks { owner: crate::MarksOwner::Sequence(f.seq), marks: m.clone() });
    m.in_point = Some(s(9));
    assert_eq!(
        Command::SetMarks { owner: crate::MarksOwner::Sequence(f.seq), marks: m }.apply(&f.p).unwrap_err(),
        CommandError::Invalid(ModelError::BadMarks),
        "in after out"
    );
    run(&f, Command::SetAssetMedia { asset: f.asset, media: MediaRef("file:moved/a".into()), info: None });
    run(&f, Command::SetTrackLayout { sequence: f.seq, track: f.a1, layout: ChannelLayout::Mono });
}

#[test]
fn frame_hold_freezes_from_the_cut() {
    let mut f = fixture();
    let a = clip(&f, 0, 10, 8);
    put(&mut f, true, &a);
    let p = run(&f, frame_hold(&f.p, a.id, s(3)).unwrap());
    let clips = p.sequence(f.seq).unwrap().track(f.v1).unwrap().1.clips.clone();
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[1].source_time(s(3)), s(13));
    assert_eq!(clips[1].source_time(s(7)), s(13), "held");
    assert_eq!(clips[0].source_time(s(2)), s(12), "before the cut it plays");
}
