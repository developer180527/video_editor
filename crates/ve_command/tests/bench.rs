//! Edit-tool timing on a long track (run with --release --ignored --nocapture).

use std::sync::Arc;
use ve_command::*;
use ve_model::*;
use ve_time::{Time, TimeRange};

#[test]
#[ignore = "timing measurement; run with --release --ignored --nocapture"]
fn bench_ripple() {
    let mut p = Project::new("t");
    let asset = Asset { id: AssetId::new(), name: "a".into(), media: MediaRef("a".into()), info: None };
    let mut v1 = Track::new(TrackKind::Video, "V1");
    let n = 6000;
    for i in 0..n {
        v1.clips.push_back(Arc::new(Clip {
            id: ClipId::new(), name: "c".into(), source: ClipSource::Asset { asset: asset.id, audio_stream: 0 },
            source_range: TimeRange::new(Time::ZERO, Time::from_seconds(1)), timeline_start: Time::from_seconds(i),
            enabled: true, link: None, effects: Default::default(),
        }));
    }
    let first = v1.clips[0].id;
    p.assets.insert(asset.id, Arc::new(asset));
    let seq = Sequence { id: SequenceId::new(), name: "s".into(), format: SequenceFormat::default(), tracks: [Arc::new(v1)].into_iter().collect() };
    p.sequences.insert(seq.id, Arc::new(seq));

    let t = std::time::Instant::now();
    let cmd = edit::ripple_delete(&p, &[first], false).unwrap();
    let built = t.elapsed();
    let t = std::time::Instant::now();
    let _ = cmd.apply(&p).unwrap();
    eprintln!("BENCH {n} clips: build {:?}, apply {:?}", built, t.elapsed());
}
