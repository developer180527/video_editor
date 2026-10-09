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
    })
}

fn sample() -> (Project, AssetId) {
    let mut p = Project::new("t");
    let a = Asset { id: AssetId::new(), name: "a.mp4".into(), media: MediaRef("file:a.mp4".into()), info: None };
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
