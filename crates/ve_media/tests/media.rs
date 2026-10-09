//! Against real media made by the `ffmpeg` CLI (skipped when it is missing).

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use platform_headless::FileStorage;
use ve_media::*;
use ve_model::*;
use ve_time::{Rate, Time, TimeRange};

/// Make a test clip with the `ffmpeg` CLI, once. Tests run in parallel: a
/// lock makes the others wait for the first, and ffmpeg writes to a
/// temporary name that is renamed into place only when complete, so no test
/// (or other test binary) ever opens a half-written file.
fn generated(name: &str, args: &[&str]) -> Option<PathBuf> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    if p.exists() {
        return Some(p);
    }
    let ext = p.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let partial = p.with_extension(format!("partial.{}.{ext}", std::process::id()));
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error"])
        .args(args)
        .arg(&partial)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        let _ = std::fs::remove_file(&partial);
        return None;
    }
    std::fs::rename(&partial, &p).ok()?;
    Some(p)
}

fn clip_file() -> Option<PathBuf> {
    generated(
        "media_test.mov",
        &[
            "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25:duration=4",
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=4",
            "-c:v", "mpeg4", "-q:v", "3", "-c:a", "pcm_s16le", "-shortest",
        ],
    )
}

fn storage() -> Arc<FileStorage> {
    Arc::new(FileStorage::rooted(env!("CARGO_TARGET_TMPDIR")))
}

#[test]
fn video_pool_decodes_ahead_and_seeks() {
    let Some(file) = clip_file() else { return eprintln!("skipped: no ffmpeg CLI") };
    let pool = VideoPool::new(storage(), Arc::new(media_ffmpeg::Ffmpeg::new()), 64 << 20);
    let media = MediaRef(format!("file:{}", file.display()));
    let r = Rate::FPS_25;
    // Exact frame, waiting for it.
    let f = pool.frame_blocking(&media, r.frame_to_time(10), Duration::from_secs(5)).unwrap();
    assert_eq!(f.pts, r.frame_to_time(10));
    // The worker decodes ahead: frame 20 arrives without asking for it.
    std::thread::sleep(Duration::from_millis(300));
    assert!(matches!(pool.frame(&media, r.frame_to_time(20)), Lookup::Exact(_)), "decoded ahead");
    // A far jump seeks; meanwhile the nearest frame is offered.
    let t = r.frame_to_time(90);
    match pool.frame(&media, t) {
        Lookup::Exact(_) | Lookup::Nearest(_) => {}
        _ => panic!("something to show while seeking"),
    }
    let f = pool.frame_blocking(&media, t, Duration::from_secs(5)).unwrap();
    assert_eq!(f.pts, t);
    // And back again.
    let f = pool.frame_blocking(&media, r.frame_to_time(3), Duration::from_secs(5)).unwrap();
    assert_eq!(f.pts, r.frame_to_time(3));
}

#[test]
fn missing_media_fails_cleanly() {
    let pool = VideoPool::new(storage(), Arc::new(media_ffmpeg::Ffmpeg::new()), 1 << 20);
    let r = pool.frame_blocking(&MediaRef("file:/nope.mov".into()), Time::ZERO, Duration::from_secs(2));
    assert!(r.is_err());
}

/// A project with the test clip's audio on A1 and A2.
fn project(file: &std::path::Path) -> (Project, SequenceId) {
    let mut p = Project::new("t");
    let asset = Asset {
        id: AssetId::new(),
        name: "a".into(),
        media: MediaRef(format!("file:{}", file.display())),
        info: Some(MediaInfo { duration: Time::from_seconds(4), video: None, audio: Vec::new() }),
        variants: Vec::new(),
        marks: Default::default(),
    };
    let clip = |track_gain_db: f64| {
        let mut params = OrdMap::new();
        params.insert("level".to_string(), Param::Constant(Value::Float(track_gain_db)));
        Arc::new(Clip {
            id: ClipId::new(),
            name: "c".into(),
            source: ClipSource::Asset { asset: asset.id, audio_stream: 0 },
            source_range: TimeRange::new(Time::ZERO, Time::from_seconds(4)),
            timeline_start: Time::ZERO,
            enabled: true,
            link: None,
            effects: [Arc::new(Effect {
                id: EffectId::new(),
                plugin: PluginRef { api: PluginApi::Builtin, id: "ve.volume".into(), major_version: 1 },
                enabled: true,
                params,
            })]
            .into_iter()
            .collect(),
            retime: Default::default(),
            transition_in: None,
            transition_out: None,
            channels: Vec::new(),
        })
    };
    let mut a1 = Track::new(TrackKind::Audio, "A1");
    a1.clips.push_back(clip(0.0));
    let mut a2 = Track::new(TrackKind::Audio, "A2");
    a2.clips.push_back(clip(-96.0)); // silent by volume
    let seq = Sequence { id: SequenceId::new(), name: "s".into(), format: SequenceFormat::default(), tracks: [a1, a2].into_iter().map(Arc::new).collect(), marks: Default::default() };
    let id = seq.id;
    p.assets.insert(asset.id, Arc::new(asset));
    p.sequences.insert(id, Arc::new(seq));
    (p, id)
}

fn peak(buf: &[f32], ch: usize) -> f32 {
    buf.chunks(2).map(|f| f[ch].abs()).fold(0.0, f32::max)
}

#[test]
fn mixer_honours_volume_mute_and_solo() {
    let Some(file) = clip_file() else { return eprintln!("skipped: no ffmpeg CLI") };
    let (mut p, seq_id) = project(&file);
    let mut m = Mixer::new(storage(), Arc::new(media_ffmpeg::Ffmpeg::new()), 44_100);
    let mut buf = vec![0f32; 4410 * 2];
    let render = |m: &mut Mixer, p: &Project, at: i64, buf: &mut Vec<f32>| {
        let seq = p.sequence(seq_id).unwrap().clone();
        m.render(p, &seq, Time::from_seconds(at), buf);
    };
    render(&mut m, &p, 1, &mut buf);
    let level = peak(&buf, 0);
    assert!((0.08..0.2).contains(&level), "A1 at 0 dB plays the 1/8 sine: {level}");
    assert!((peak(&buf, 1) - level).abs() < 0.01, "centred");

    // Mute A1: only A2 is left, and it is at -96 dB.
    let set = |p: &mut Project, i: usize, f: &dyn Fn(&mut Track)| {
        let s = Arc::make_mut(p.sequences.get_mut(&seq_id).unwrap());
        f(Arc::make_mut(&mut s.tracks[i]));
    };
    set(&mut p, 0, &|t| t.muted = true);
    render(&mut m, &p, 2, &mut buf);
    assert_eq!(peak(&buf, 0), 0.0);

    // Solo A2 with A1 unmuted: still silent (A2 is at -96 dB).
    set(&mut p, 0, &|t| t.muted = false);
    set(&mut p, 1, &|t| t.solo = true);
    render(&mut m, &p, 3, &mut buf);
    assert_eq!(peak(&buf, 0), 0.0);
}

#[test]
fn playback_callback_delivers_mixed_audio() {
    let Some(file) = clip_file() else { return eprintln!("skipped: no ffmpeg CLI") };
    let (p, seq_id) = project(&file);
    let mixer = Mixer::new(storage(), Arc::new(media_ffmpeg::Ffmpeg::new()), 48_000);
    let (pb, mut callback) = Playback::new(mixer, 2);
    pb.play(Arc::new(p), seq_id, Time::from_seconds(1));
    std::thread::sleep(Duration::from_millis(200)); // let the mixer run ahead
    let mut out = vec![0f32; 512 * 2];
    callback(&mut out);
    assert!(peak(&out, 0) > 0.05, "sound comes out");
    assert!(pb.meters.peaks()[0] > 0.05, "and the meters see it");
    pb.stop();
    callback(&mut out);
    assert_eq!(peak(&out, 0), 0.0, "silence after stop");
}

#[test]
fn stills_make_thumbnails_and_peaks() {
    let Some(file) = clip_file() else { return eprintln!("skipped: no ffmpeg CLI") };
    let stills = Stills::new(storage(), Arc::new(media_ffmpeg::Ffmpeg::new()));
    let (asset, media) = (AssetId::new(), MediaRef(format!("file:{}", file.display())));
    assert!(stills.thumb(asset, &media, Time::from_seconds(1)).is_none(), "queued, not ready");
    assert!(stills.peaks(asset, &media, 0).is_none());
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let (thumb, peaks) = loop {
        if let (Some(t), Some(p)) = (stills.thumb(asset, &media, Time::from_seconds(1)), stills.peaks(asset, &media, 0)) {
            break (t.1, p);
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!((thumb.width, thumb.height), (160, 90));
    assert_eq!(thumb.rgba.len(), 160 * 90 * 4);
    assert!(thumb.rgba.chunks(4).any(|p| p[0] > 200 || p[1] > 200 || p[2] > 200), "testsrc2 has bright colours");
    // 4 s at 100 peaks/s; the sine is 1/8 amplitude.
    assert!((395..=401).contains(&peaks.len()), "{}", peaks.len());
    let max = peaks.iter().cloned().fold(0.0, f32::max);
    assert!((0.1..0.15).contains(&max), "{max}");
    // A run along the clip (one decoder, sought each time, backwards too):
    // every slot gets its own picture.
    let mut got = std::collections::HashMap::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while got.len() < 4 {
        for s in [3, 0, 2] {
            if let Some((_, t)) = stills.thumb(asset, &media, Time::from_seconds(s)) {
                got.insert(s, t.rgba.clone());
            }
        }
        got.insert(1, thumb.rgba.clone());
        assert!(std::time::Instant::now() < deadline, "thumbnails never came");
        std::thread::sleep(Duration::from_millis(10));
    }
    for (a, b) in [(0, 1), (1, 2), (2, 3), (0, 3)] {
        assert_ne!(got[&a], got[&b], "slots {a} and {b} show the same picture");
    }
}

/// Real-time playback against the cache: at 60 Hz for 4 s, ask for the frame
/// at wall-clock time and count misses (the frame was not decoded yet).
#[test]
#[ignore = "timing measurement; run with --ignored --nocapture"]
fn realtime_playback_keeps_up() {
    let Some(p) = generated(
        "rt_1080.mp4",
        &["-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=24:duration=8", "-c:v", "libx264", "-pix_fmt", "yuv420p", "-g", "48"],
    ) else {
        return eprintln!("skipped");
    };
    let pool = VideoPool::new(storage(), Arc::new(media_ffmpeg::Ffmpeg::new()), 512 << 20);
    let media = MediaRef(format!("file:{}", p.display()));
    pool.frame_blocking(&media, Time::ZERO, Duration::from_secs(5)).unwrap();
    let start = std::time::Instant::now();
    let (mut exact, mut miss) = (0, 0);
    let mut worst = Duration::ZERO;
    while start.elapsed() < Duration::from_secs(4) {
        let t = Time::from_seconds_f64(start.elapsed().as_secs_f64());
        let q = std::time::Instant::now();
        match pool.frame(&media, t) {
            Lookup::Exact(_) => exact += 1,
            _ => miss += 1,
        }
        worst = worst.max(q.elapsed());
        std::thread::sleep(Duration::from_micros(16_667));
    }
    println!("exact {exact}, missed {miss}, slowest lookup {worst:?}");
}

/// Storage whose `resolve` waits until the gate opens: holds the mixer
/// while it opens a clip.
struct Gated {
    open: std::sync::Mutex<bool>,
    cv: std::sync::Condvar,
}

impl ve_ports::Storage for Gated {
    fn make_ref(&self, p: &str) -> Result<MediaRef, ve_ports::StorageError> {
        Ok(MediaRef(p.into()))
    }
    fn resolve(&self, r: &MediaRef) -> Result<ve_ports::Resolved, ve_ports::StorageError> {
        let _g = self.cv.wait_while(self.open.lock().unwrap(), |o| !*o).unwrap();
        Err(ve_ports::StorageError::NotFound(r.0.clone()))
    }
    fn resolve_new(&self, r: &MediaRef) -> Result<ve_ports::Resolved, ve_ports::StorageError> {
        self.resolve(r)
    }
    fn open_read(&self, r: &MediaRef) -> Result<Box<dyn ve_ports::ReadSeek>, ve_ports::StorageError> {
        Err(ve_ports::StorageError::NotFound(r.0.clone()))
    }
    fn open_write(&self, r: &MediaRef) -> Result<Box<dyn std::io::Write + Send>, ve_ports::StorageError> {
        Err(ve_ports::StorageError::NotFound(r.0.clone()))
    }
    fn location(&self, _: ve_ports::Location, n: &str) -> Result<MediaRef, ve_ports::StorageError> {
        Ok(MediaRef(n.into()))
    }
    fn display_name(&self, r: &MediaRef) -> String {
        r.0.clone()
    }
}

/// The playhead clock counts sound actually delivered: nothing while the
/// mixer is still opening its sources, whole buffers once it runs.
#[test]
fn playback_clock_waits_for_the_mixer() {
    let gate = Arc::new(Gated { open: std::sync::Mutex::new(false), cv: std::sync::Condvar::new() });
    let mixer = Mixer::new(gate.clone(), Arc::new(platform_headless::NoMedia), 48_000);
    let (pb, mut callback) = Playback::new(mixer, 2);
    let clock = pb.clock();
    let mut buf = vec![0f32; 512 * 2];
    let (p, seq) = project(&PathBuf::from("/nowhere.mov"));
    // Stopped: silence, and the clock stands still.
    callback(&mut buf);
    assert_eq!(clock.read().0, 0);
    // Started, but the mixer is stuck opening the clip: device silence is
    // not playback.
    pb.play(Arc::new(p), seq, Time::ZERO);
    std::thread::sleep(Duration::from_millis(50));
    callback(&mut buf);
    assert_eq!(clock.read().0, 0, "silence before the mixer delivers is not playback");
    // Once the mixer runs, whole buffers count.
    *gate.open.lock().unwrap() = true;
    gate.cv.notify_all();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while clock.read().0 == 0 {
        assert!(std::time::Instant::now() < deadline, "mixer never delivered");
        std::thread::sleep(Duration::from_millis(5));
        callback(&mut buf);
    }
    assert_eq!(clock.read().0 % 512, 0);
    let n = clock.read().0;
    pb.stop();
    callback(&mut buf);
    assert_eq!(clock.read().0, n, "stopped: the clock stands still");
}

/// With frames left in GPU memory, the cache holds the decoder's own
/// buffers: decoding must keep going while a second of them is held.
#[test]
fn native_frames_flow_through_the_cache() {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("native_cache.mp4");
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=30:duration=4", "-c:v", "h264_videotoolbox"])
        .arg(&p)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return eprintln!("skipped: no ffmpeg CLI with VideoToolbox");
    }
    let pool = VideoPool::new(storage(), Arc::new(media_ffmpeg::Ffmpeg::new()), 512 << 20);
    pool.set_gpu_frames(true);
    let media = MediaRef(format!("file:{}", p.display()));
    let r = Rate::FPS_30;
    let mut native = 0;
    for i in 0..120 {
        let f = pool.frame_blocking(&media, r.frame_to_time(i), Duration::from_secs(5)).unwrap();
        assert_eq!(f.pts.to_frame(r), i);
        native += matches!(f.data, ve_ports::FrameData::Native(_)) as usize;
    }
    eprintln!("{native}/120 frames stayed in GPU memory");
}

/// A clip set to the file's second audio stream plays that stream.
#[test]
fn clips_play_their_own_audio_stream() {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("two_streams.mov");
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2"])
        .args(["-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000:duration=2"])
        .args(["-map", "0", "-map", "1", "-c:a", "pcm_s16le"])
        .arg(&p)
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return eprintln!("skipped: no ffmpeg CLI");
    }
    let (mut proj, seq_id) = project(&p);
    // Point every clip at stream 2 (index 1).
    let mut seq = (*proj.sequences[&seq_id]).clone();
    for t in seq.tracks.iter_mut() {
        let t = Arc::make_mut(t);
        for c in t.clips.iter_mut() {
            let c = Arc::make_mut(c);
            if let ClipSource::Asset { audio_stream, .. } = &mut c.source {
                *audio_stream = 1;
            }
        }
    }
    proj.sequences.insert(seq_id, Arc::new(seq.clone()));
    let mut mixer = Mixer::new(storage(), Arc::new(media_ffmpeg::Ffmpeg::new()), 48_000);
    let mut out = vec![0f32; 48_000 * 2];
    mixer.render(&proj, &seq, Time::ZERO, &mut out);
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    let crossings = left.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count() as f64;
    assert!((crossings / 2.0 - 880.0).abs() < 5.0, "heard {} Hz", crossings / 2.0);
}
