use super::*;
use ve_time::{Time, TimeRange};

fn s(x: i64) -> Time {
    Time::from_seconds(x)
}

fn clip(asset: AssetId, start: i64, len: i64) -> Arc<Clip> {
    Arc::new(Clip {
        id: ClipId::new(),
        name: "c".into(),
        source: ClipSource::Asset { asset, audio_stream: 0 },
        source_range: TimeRange::new(s(0), s(len)),
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

fn sample() -> (Project, AssetId) {
    let mut p = Project::new("t");
    let a = Asset { id: AssetId::new(), name: "a.mp4".into(), media: MediaRef("file:a.mp4".into()), info: None, variants: Vec::new(), marks: Default::default() };
    let aid = a.id;
    p.assets.insert(aid, Arc::new(a));
    let mut v1 = Track::new(TrackKind::Video, "V1");
    v1.clips.push_back(clip(aid, 0, 5));
    v1.clips.push_back(clip(aid, 5, 3));
    let seq = Sequence {
        id: SequenceId::new(),
        name: "Sequence 01".into(),
        format: SequenceFormat::default(),
        tracks: [Arc::new(v1), Arc::new(Track::new(TrackKind::Audio, "A1"))].into_iter().collect(),
        marks: Default::default(),
    };
    p.active_sequence = Some(seq.id);
    p.sequences.insert(seq.id, Arc::new(seq));
    (p, aid)
}

#[test]
fn sample_is_valid() {
    let (p, _) = sample();
    assert_eq!(validate(&p), Ok(()));
    assert_eq!(p.active().unwrap().duration(), s(8));
}

#[test]
fn overlap_is_rejected() {
    let (mut p, aid) = sample();
    let seq = Arc::make_mut(p.sequences.get_mut(&p.active_sequence.unwrap()).unwrap());
    Arc::make_mut(&mut seq.tracks[0]).clips.push_back(clip(aid, 7, 2));
    assert!(matches!(validate(&p), Err(ModelError::Overlap(..))));
}

#[test]
fn clip_lookup_is_half_open() {
    let (p, _) = sample();
    let t = &p.active().unwrap().tracks[0];
    assert_eq!(t.clip_at(s(0)).unwrap().id, t.clips[0].id);
    assert_eq!(t.clip_at(s(5)).unwrap().id, t.clips[1].id);
    assert!(t.clip_at(s(8)).is_none());
}

#[test]
fn snapshots_share_structure() {
    let (p, _) = sample();
    let before = Arc::new(p.clone());
    let mut after = p;
    after.name = "renamed".into();
    // The untouched sequence is the same allocation in both versions.
    let id = before.active_sequence.unwrap();
    assert!(Arc::ptr_eq(&before.sequences[&id], &after.sequences[&id]));
}

#[test]
fn json_round_trip() {
    let (p, _) = sample();
    let text = serde_json::to_string(&p).unwrap();
    let back: Project = serde_json::from_str(&text).unwrap();
    assert_eq!(p, back);
}

#[test]
fn keyframes_interpolate() {
    let p = Param::Animated(vec![
        Keyframe { time: s(0), value: Value::Float(0.0), interp: Interp::Linear },
        Keyframe { time: s(10), value: Value::Float(100.0), interp: Interp::Hold },
    ]);
    assert_eq!(p.value_at(s(5)), Value::Float(50.0));
    assert_eq!(p.value_at(s(20)), Value::Float(100.0));
    assert_eq!(p.value_at(-s(1)), Value::Float(0.0));
}

/// `is_free` only looks at neighbours; it must agree with checking every clip.
#[test]
fn is_free_agrees_with_a_full_scan() {
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut rnd = |n: i64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % n as u64) as i64
    };
    let t = |x: i64| Time::from_ticks(x);
    let mut track = Track::new(TrackKind::Video, "V1");
    let mut at = 0;
    for _ in 0..200 {
        at += rnd(5);
        let len = 1 + rnd(6);
        track.clips.push_back(Arc::new(Clip {
            id: ClipId::new(),
            name: "c".into(),
            source: ClipSource::Generator { plugin: PluginRef { api: PluginApi::Builtin, id: "g".into(), major_version: 1 } },
            source_range: TimeRange::new(t(0), t(len)),
            timeline_start: t(at),
            enabled: true,
            link: None,
            effects: Default::default(),
            retime: Default::default(),
            transition_in: None,
            transition_out: None,
            channels: Vec::new(),
        }));
        at += len;
    }
    for _ in 0..20_000 {
        let range = TimeRange::new(t(rnd(at + 10) - 5), t(1 + rnd(12)));
        let except = (rnd(3) == 0).then(|| track.clips[rnd(200) as usize].id);
        let full = track.clips.iter().all(|c| Some(c.id) == except || !c.timeline_range().overlaps(range));
        assert_eq!(track.is_free(range, except), full, "{range:?} except {except:?}");
    }
}

fn dissolve(before: i64, after: i64) -> Option<Arc<Transition>> {
    Some(Arc::new(Transition {
        id: EffectId::new(),
        plugin: PluginRef { api: PluginApi::Builtin, id: "ve.dissolve".into(), major_version: 1 },
        before: s(before),
        after: s(after),
        params: OrdMap::new(),
    }))
}

#[test]
fn transitions_decide_what_plays() {
    let asset = AssetId::new();
    let mut a = (*clip(asset, 0, 10)).clone();
    let mut b = (*clip(asset, 10, 10)).clone(); // adjacent: a cut at 10
    let mut c = (*clip(asset, 30, 10)).clone(); // after a gap
    b.transition_in = dissolve(1, 1); // cross-dissolve 9..11
    c.transition_in = dissolve(0, 2); // fade in 30..32
    c.transition_out = dissolve(2, 0); // fade out 38..40
    a.transition_out = dissolve(5, 5); // ignored: b follows directly
    let mut t = Track::new(TrackKind::Video, "V1");
    for x in [a.clone(), b.clone(), c.clone()] {
        t.clips.push_back(Arc::new(x));
    }
    let at = |sec: f64| t.active_at(Time::from_seconds_f64(sec));
    let roles = |v: Vec<Active>| v.iter().map(|x| (x.clip.id, x.transition.map(|(_, p, r)| ((p * 100.0).round() as i32, r)))).collect::<Vec<_>>();
    assert_eq!(roles(at(5.0)), [(a.id, None)]);
    assert_eq!(roles(at(9.5)), [(a.id, Some((25, Role::Outgoing))), (b.id, Some((25, Role::Incoming)))], "a cross-dissolve");
    assert_eq!(roles(at(10.5)), [(a.id, Some((75, Role::Outgoing))), (b.id, Some((75, Role::Incoming)))]);
    assert_eq!(roles(at(15.0)), [(b.id, None)], "a's fade out is ignored at the cut");
    assert_eq!(roles(at(31.0)), [(c.id, Some((50, Role::Incoming)))], "a fade from nothing");
    assert_eq!(roles(at(39.0)), [(c.id, Some((50, Role::Outgoing)))], "a fade to nothing");
    assert!(at(25.0).is_empty());
    // The clips reach into their handles for the dissolve.
    assert_eq!(t.reach(0), TimeRange::from_bounds(s(0), s(11)));
    assert_eq!(t.reach(1), TimeRange::from_bounds(s(9), s(20)));
}

#[test]
fn transition_regions_may_not_overlap() {
    let asset = AssetId::new();
    let (mut p, seq_id) = {
        let mut p = Project::new("t");
        p.assets.insert(asset, Arc::new(Asset::new("a", MediaRef("file:a".into()), None)));
        let seq = Sequence::new("s", SequenceFormat::default(), [Track::new(TrackKind::Video, "V1")]);
        let id = seq.id;
        p.sequences.insert(id, Arc::new(seq));
        (p, id)
    };
    let mut asset_obj = (*p.assets[&asset]).clone();
    asset_obj.id = asset;
    p.assets.insert(asset, Arc::new(asset_obj));
    let put = |p: &mut Project, clips: Vec<Clip>| {
        let mut seq = (*p.sequences[&seq_id]).clone();
        let t = Arc::make_mut(&mut seq.tracks[0]);
        t.clips = clips.into_iter().map(Arc::new).collect();
        p.sequences.insert(seq_id, Arc::new(seq));
    };
    let a = (*clip(asset, 0, 10)).clone();
    let mut b = (*clip(asset, 10, 10)).clone();
    b.transition_in = dissolve(4, 1);
    put(&mut p, vec![a.clone(), b.clone()]);
    assert_eq!(validate(&p), Ok(()));
    // Reaching back past the start of the clip before it.
    b.transition_in = dissolve(11, 1);
    put(&mut p, vec![a, b]);
    assert!(matches!(validate(&p), Err(ModelError::BadTransition(_))));
}

#[test]
fn bezier_keyframes_follow_their_curve() {
    let key = |t: i64, v: f64, interp: Interp| Keyframe { time: Time::from_seconds(t), value: Value::Float(v), interp };
    let at = |p: &Param, ms: i64| match p.value_at(Time::from_ticks(ms * ve_time::TICKS_PER_SECOND / 1000)) {
        Value::Float(v) => v,
        _ => panic!(),
    };
    // The linear-equivalent curve is linear.
    let lin = Interp::Linear.as_bezier().unwrap();
    let p = Param::Animated(vec![key(0, 0.0, Interp::Bezier { x1: lin.0, y1: lin.1, x2: lin.2, y2: lin.3 }), key(10, 100.0, Interp::Linear)]);
    for ms in [0, 1000, 2500, 5000, 7500, 9999] {
        assert!((at(&p, ms) - ms as f64 / 100.0).abs() < 1e-4, "{ms}: {}", at(&p, ms));
    }
    // Ease in-out: slow at both ends, half-way at the middle, symmetric.
    let p = Param::Animated(vec![key(0, 0.0, Interp::EASE_IN_OUT), key(10, 100.0, Interp::Linear)]);
    assert!(at(&p, 1000) < 5.0, "{}", at(&p, 1000));
    assert!((at(&p, 5000) - 50.0).abs() < 1e-3);
    assert!((at(&p, 2000) + at(&p, 8000) - 100.0).abs() < 1e-3);
    // Handles above 1 overshoot, then settle on the next key.
    let p = Param::Animated(vec![key(0, 0.0, Interp::Bezier { x1: 0.3, y1: 1.6, x2: 0.6, y2: 1.3 }), key(10, 100.0, Interp::Linear)]);
    assert!((0..10_000).step_by(250).any(|ms| at(&p, ms) > 100.0), "overshoots");
    assert_eq!(at(&p, 10_000), 100.0);
}

#[test]
fn a_curve_timed_outside_its_segment_is_refused() {
    let with = |interp: Interp| {
        let (mut p, _) = sample();
        let sid = *p.sequences.keys().next().unwrap();
        let seq = Arc::make_mut(p.sequences.get_mut(&sid).unwrap());
        let track = Arc::make_mut(&mut seq.tracks[0]);
        let clip = Arc::make_mut(&mut track.clips[0]);
        let mut params = OrdMap::new();
        params.insert("x".to_string(), Param::Animated(vec![Keyframe { time: Time::ZERO, value: Value::Float(0.0), interp }]));
        let plugin = PluginRef { api: PluginApi::Builtin, id: "fx".into(), major_version: 1 };
        clip.effects.push_back(Arc::new(Effect { id: EffectId::new(), plugin, enabled: true, params }));
        validate(&p)
    };
    assert_eq!(with(Interp::EASE_IN_OUT), Ok(()));
    assert_eq!(with(Interp::Bezier { x1: 0.3, y1: 2.0, x2: 0.6, y2: -1.0 }), Ok(()), "values may overshoot");
    assert!(matches!(with(Interp::Bezier { x1: 1.5, y1: 0.0, x2: 0.5, y2: 1.0 }), Err(ModelError::BadKeyframes(_))), "time runs backwards");
    assert!(matches!(with(Interp::Bezier { x1: 0.2, y1: f32::NAN, x2: 0.5, y2: 1.0 }), Err(ModelError::BadKeyframes(_))));
}
