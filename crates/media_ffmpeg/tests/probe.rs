//! Probes a clip made by the `ffmpeg` command-line tool, when one is
//! installed; skips otherwise.

use std::path::PathBuf;
use std::process::Command;
use media_ffmpeg::Ffmpeg;
use ve_ports::{MediaBackend, Resolved};
use ve_time::{Rate, Time};

#[test]
fn version_is_the_pinned_one() {
    let v = Ffmpeg::version();
    assert!(v.trim_start_matches('n').starts_with("9."), "{v}");
}

#[test]
fn probe_generated_clip() {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let clip = dir.join("probe_test.mp4");
    let made = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25:duration=2"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2"])
        .args(["-c:v", "mpeg4", "-c:a", "aac", "-shortest"])
        .arg(&clip)
        .status();
    if !matches!(made, Ok(s) if s.success()) {
        eprintln!("skipped: no ffmpeg CLI to make a test clip");
        return;
    }
    let info = Ffmpeg::new().probe(&Resolved { path: Some(clip), guard: Box::new(()) }).unwrap();
    let v = info.video.unwrap();
    assert_eq!((v.width, v.height, v.rate), (320, 180, Rate::FPS_25));
    let a = info.audio.unwrap();
    assert_eq!(a.sample_rate, 48000);
    let secs = info.duration.as_seconds_f64();
    assert!((1.9..2.2).contains(&secs), "{secs}");
    assert!(info.duration > Time::ZERO);
}

#[test]
fn missing_file_is_an_error() {
    let r = Resolved { path: Some("/nonexistent/x.mov".into()), guard: Box::new(()) };
    assert!(Ffmpeg::new().probe(&r).is_err());
}

fn make_clip(name: &str, extra: &[&str]) -> Option<PathBuf> {
    let clip = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25:duration=2"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100:duration=2"])
        .args(extra)
        .args(["-shortest"])
        .arg(&clip)
        .status()
        .is_ok_and(|s| s.success());
    ok.then_some(clip)
}

#[test]
fn decode_video_frames_and_seek() {
    let Some(clip) = make_clip("decode_h264.mp4", &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-c:a", "aac"])
        .or_else(|| make_clip("decode_mpeg4.mp4", &["-c:v", "mpeg4", "-c:a", "aac"]))
    else {
        eprintln!("skipped: no ffmpeg CLI");
        return;
    };
    let r = Resolved { path: Some(clip), guard: Box::new(()) };
    let mut d = Ffmpeg::new().open_video(&r).unwrap();
    let f = d.next_frame().unwrap().unwrap();
    assert_eq!((f.width, f.height, f.format), (320, 180, ve_ports::PixelFormat::Nv12));
    assert_eq!(f.pts, Time::ZERO);
    let ve_ports::FrameData::Cpu { planes, strides } = &f.data else { panic!("cpu frame") };
    assert_eq!(planes.len(), 2);
    assert!(strides[0] >= 320 && planes[0].len() >= 320 * 180);
    // Seek to frame 30 (1.2 s): the next frame returned is that one.
    let t = Rate::FPS_25.frame_to_time(30);
    d.seek(t).unwrap();
    let f = d.next_frame().unwrap().unwrap();
    assert_eq!(f.pts, t);
    let mut n = 1;
    while d.next_frame().unwrap().is_some() {
        n += 1;
    }
    assert_eq!(n, 20, "frames 30..50 remain");
}

#[test]
fn decode_audio_resampled() {
    let Some(clip) = make_clip("decode_audio.mp4", &["-c:v", "mpeg4", "-c:a", "aac"]) else {
        eprintln!("skipped: no ffmpeg CLI");
        return;
    };
    let r = Resolved { path: Some(clip), guard: Box::new(()) };
    let mut d = Ffmpeg::new().open_audio(&r, 48_000, 2).unwrap();
    let mut total = 0usize;
    let mut peak = 0f32;
    while let Some(b) = d.next_block().unwrap() {
        assert_eq!((b.sample_rate, b.channels), (48_000, 2));
        total += b.samples.len() / 2;
        peak = b.samples.iter().fold(peak, |m, s| m.max(s.abs()));
    }
    let secs = total as f64 / 48_000.0;
    assert!((1.9..2.2).contains(&secs), "{secs} s of audio");
    assert!(peak > 0.05, "the sine is there (lavfi sine is 1/8 amplitude): peak {peak}");
    // Seek: the first block starts at the target.
    d.seek(Time::from_seconds(1)).unwrap();
    let b = d.next_block().unwrap().unwrap();
    assert_eq!(b.pts, Time::from_seconds(1));
}

fn encode(codec: &str, container: &str) -> ve_model::MediaInfo {
    use ve_ports::*;
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("encode_test.{container}"));
    let settings = EncoderSettings {
        video_codec: codec.into(),
        audio_codec: "aac".into(),
        container: container.into(),
        width: 320,
        height: 180,
        rate: Rate::FPS_25,
        sample_rate: 48_000,
        channels: 2,
        video_bitrate: None,
        prefer_hardware: true,
    };
    let out = Resolved { path: Some(path.clone()), guard: Box::new(()) };
    let mut enc = Ffmpeg::new().open_encoder(&out, &settings).unwrap();
    for i in 0..50u32 {
        let px: Vec<u8> = (0..320 * 180).flat_map(|p| [(p % 320) as u8, (i * 5) as u8, 128, 255]).collect();
        enc.push_video(VideoFrame {
            pts: Rate::FPS_25.frame_to_time(i as i64),
            duration: Rate::FPS_25.frame_duration(),
            width: 320,
            height: 180,
            format: PixelFormat::Rgba8,
            color: ColorTags::default(),
            data: FrameData::Cpu { planes: vec![px], strides: vec![320 * 4] },
        })
        .unwrap();
    }
    let samples: Vec<f32> = (0..96_000).flat_map(|i| {
        let s = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
        [s, s]
    }).collect();
    enc.push_audio(AudioBlock { pts: Time::ZERO, sample_rate: 48_000, channels: 2, samples }).unwrap();
    enc.finish().unwrap();
    Ffmpeg::new().probe(&Resolved { path: Some(path), guard: Box::new(()) }).unwrap()
}

#[test]
fn encode_h264_mp4_round_trips() {
    let info = encode("h264", "mp4");
    let v = info.video.unwrap();
    assert_eq!((v.width, v.height, v.rate, v.codec.as_str()), (320, 180, Rate::FPS_25, "h264"));
    assert_eq!(info.audio.unwrap().codec, "aac");
    let secs = info.duration.as_seconds_f64();
    assert!((1.9..2.2).contains(&secs), "{secs}");
}

#[test]
fn encode_prores_mov_round_trips() {
    let info = encode("prores", "mov");
    assert_eq!(info.video.unwrap().codec, "prores");
}
