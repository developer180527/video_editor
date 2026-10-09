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
    let a = &info.audio[0];
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
    let ve_ports::CpuPlanes { planes, strides } = f.data.cpu().expect("cpu frame");
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
    let mut d = Ffmpeg::new().open_audio(&r, 0, 48_000, 2).unwrap();
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

/// `None` (and a note) when this machine has no encoder for `codec`: CI
/// runners have no GPU media engine, and H.264/HEVC are only encoded there.
fn encode(codec: &str, container: &str) -> Option<ve_model::MediaInfo> {
    encode_frames(codec, container, false).map(|(info, _)| info)
}

/// As `encode`, feeding 16-bit frames when `deep`; also returns the path.
fn encode_frames(codec: &str, container: &str, deep: bool) -> Option<(ve_model::MediaInfo, PathBuf)> {
    use ve_ports::*;
    let path = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("encode_test_{codec}{}.{container}", if deep { "_deep" } else { "" }));
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
    let mut enc = match Ffmpeg::new().open_encoder(&out, &settings) {
        Err(MediaError::Unsupported(m)) if m.contains("encoder works here") => {
            eprintln!("skipped: {m}");
            return None;
        }
        r => r.unwrap(),
    };
    for i in 0..50u32 {
        let (px, format, bpp): (Vec<u8>, _, usize) = if deep {
            let px = (0..320 * 180).flat_map(|p: u32| [((p % 320) * 200) as u16, (i * 1300) as u16, 32768, 65535]).flat_map(u16::to_le_bytes).collect();
            (px, PixelFormat::Rgba16, 8)
        } else {
            ((0..320 * 180).flat_map(|p| [(p % 320) as u8, (i * 5) as u8, 128, 255]).collect(), PixelFormat::Rgba8, 4)
        };
        enc.push_video(VideoFrame {
            pts: Rate::FPS_25.frame_to_time(i as i64),
            duration: Rate::FPS_25.frame_duration(),
            width: 320,
            height: 180,
            format,
            color: ColorTags::default(),
            data: FrameData::Cpu { planes: vec![px], strides: vec![320 * bpp] },
        })
        .unwrap();
    }
    let samples: Vec<f32> = (0..96_000).flat_map(|i| {
        let s = (i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.5;
        [s, s]
    }).collect();
    enc.push_audio(AudioBlock { pts: Time::ZERO, sample_rate: 48_000, channels: 2, samples }).unwrap();
    enc.finish().unwrap();
    Some((Ffmpeg::new().probe(&Resolved { path: Some(path.clone()), guard: Box::new(()) }).unwrap(), path))
}

/// From 16-bit frames, HEVC comes out 10-bit (Main 10) where the encoder
/// can, and ProRes always.
#[test]
fn deep_frames_encode_at_ten_bits() {
    for (codec, container) in [("prores", "mov"), ("hevc", "mp4")] {
        let Some((_, path)) = encode_frames(codec, container, true) else { continue };
        let mut d = Ffmpeg::new().open_video(&Resolved { path: Some(path), guard: Box::new(()) }).unwrap();
        let f = d.next_frame().unwrap().unwrap();
        assert_eq!(f.format, ve_ports::PixelFormat::P010, "{codec} decodes as 10-bit");
    }
}

#[test]
fn encode_h264_mp4_round_trips() {
    let Some(info) = encode("h264", "mp4") else { return };
    let v = info.video.unwrap();
    assert_eq!((v.width, v.height, v.rate, v.codec.as_str()), (320, 180, Rate::FPS_25, "h264"));
    assert_eq!(info.audio[0].codec, "aac");
    let secs = info.duration.as_seconds_f64();
    assert!((1.9..2.2).contains(&secs), "{secs}");
}

#[test]
fn encode_prores_mov_round_trips() {
    let info = encode("prores", "mov").expect("ProRes is encoded in software everywhere");
    assert_eq!(info.video.unwrap().codec, "prores");
}

/// A clip made by the `ffmpeg` CLI into the test temp dir, or `None`.
fn made(name: &str, args: &[&str]) -> Option<Resolved> {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let ok = Command::new("ffmpeg").args(["-y", "-loglevel", "error"]).args(args).arg(&p).status().is_ok_and(|s| s.success());
    ok.then(|| Resolved { path: Some(p), guard: Box::new(()) })
}

fn first_yuv(r: Resolved) -> (u8, u8, u8, ve_ports::ColorTags) {
    let mut d = Ffmpeg::new().open_video(&r).unwrap();
    let f = d.next_frame().unwrap().unwrap();
    let ve_ports::CpuPlanes { planes, strides } = f.data.cpu().unwrap();
    let (x, y) = (100, 100);
    (planes[0][y * strides[0] + x], planes[1][(y / 2) * strides[1] + x], planes[1][(y / 2) * strides[1] + x + 1], f.color)
}

/// An MPEG-TS starts its clock at 1.4 s: source time 0 is still its first
/// frame and first sound, and the whole duration is reachable.
#[test]
fn timestamps_start_at_the_file_start() {
    let Some(r) = made("start.ts", &["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25:duration=4", "-f", "lavfi", "-i", "sine=duration=4", "-c:v", "mpeg2video", "-c:a", "mp2"]) else {
        return eprintln!("skipped: no ffmpeg CLI");
    };
    let ff = Ffmpeg::new();
    let mut v = ff.open_video(&r).unwrap();
    let first = v.next_frame().unwrap().unwrap().pts;
    assert!(first.as_seconds_f64().abs() < 0.05, "first frame at {:.3} s", first.as_seconds_f64());
    // Every time in the clip, near its end too, finds the frame showing then.
    for t in [Time::from_seconds(2), Rate::FPS_25.frame_to_time(99)] {
        v.seek(t).unwrap();
        let f = v.next_frame().unwrap().unwrap();
        assert!(f.pts <= t && t < f.pts + f.duration, "{:.3} s for {:.3} s", f.pts.as_seconds_f64(), t.as_seconds_f64());
    }
    let mut a = ff.open_audio(&r, 0, 48_000, 2).unwrap();
    let b = a.next_block().unwrap().unwrap();
    assert!(b.pts.as_seconds_f64().abs() < 0.1, "first sound at {:.3} s", b.pts.as_seconds_f64());
}

/// Seeking past the end gives the last frame, not nothing.
#[test]
fn seek_past_the_end_gives_the_last_frame() {
    let Some(r) = made("short.mov", &["-f", "lavfi", "-i", "testsrc2=size=320x180:rate=25:duration=1", "-c:v", "mpeg4"]) else {
        return eprintln!("skipped: no ffmpeg CLI");
    };
    let mut v = Ffmpeg::new().open_video(&r).unwrap();
    v.seek(Time::from_seconds(5)).unwrap();
    assert_eq!(v.next_frame().unwrap().unwrap().pts.to_frame(Rate::FPS_25), 24);
}

/// RGB sources are converted with, and tagged as, BT.709 limited range.
#[test]
fn rgb_sources_are_tagged_as_converted() {
    let Some(r) = made("red.mov", &["-f", "lavfi", "-i", "color=c=red:size=1920x1080:rate=25:duration=0.2", "-c:v", "qtrle", "-pix_fmt", "rgb24"]) else {
        return eprintln!("skipped: no ffmpeg CLI");
    };
    let (y, cb, cr, tags) = first_yuv(r);
    assert_eq!(tags.matrix, "bt709");
    assert!(!tags.full_range);
    // BT.709 limited-range red is 63/102/240 (BT.601 would be 81/90/240).
    assert!((y as i32 - 63).abs() <= 1 && (cb as i32 - 102).abs() <= 1 && (cr as i32 - 240).abs() <= 1, "{y}/{cb}/{cr}");
}

/// A full-range source converted to NV12 stays full range, and says so.
#[test]
fn full_range_survives_conversion() {
    let Some(r) = made("full.mov", &["-f", "lavfi", "-i", "color=c=white:size=320x180:rate=25:duration=0.2", "-c:v", "mjpeg", "-pix_fmt", "yuvj422p"]) else {
        return eprintln!("skipped: no ffmpeg CLI");
    };
    let (y, _, _, tags) = first_yuv(r);
    assert!(tags.full_range, "{tags:?}");
    assert!(y >= 250, "white is {y} in a full-range frame");
}

/// Hardware decode (VideoToolbox here) keeps the stream's colour tags: they
/// do not travel with the pixels when frames are copied off the hardware.
#[test]
fn hardware_decode_keeps_colour_tags() {
    let Some(r) = made("hlg.mp4", &["-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=25:duration=0.4", "-c:v", "h264", "-pix_fmt", "yuv420p", "-bsf:v", "h264_metadata=colour_primaries=9:transfer_characteristics=18:matrix_coefficients=9"]) else {
        return eprintln!("skipped: no ffmpeg CLI");
    };
    let d = media_ffmpeg::VideoDec::open(&r, true).unwrap();
    let hardware = d.hardware;
    let mut d: Box<dyn ve_ports::VideoDecoder> = Box::new(d);
    let f = d.next_frame().unwrap().unwrap();
    assert_eq!((f.color.primaries.as_str(), f.color.transfer.as_str(), f.color.matrix.as_str()), ("bt2020", "arib-std-b67", "bt2020nc"), "hardware={hardware}");
    // And the planes are handed over, not copied.
    assert!(matches!(f.data, ve_ports::FrameData::Shared(_)));
}

/// Sign changes per second of a mono signal: about twice its frequency.
fn crossings(samples: &[f32], rate: u32) -> f64 {
    let n = samples.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    n as f64 * rate as f64 / samples.len() as f64
}

/// A camera-style file: two mono streams (440 Hz, 880 Hz) and a 5.1 one.
fn three_streams() -> Option<Resolved> {
    made(
        "streams.mov",
        &[
            "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=1",
            "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000:duration=1",
            "-f", "lavfi", "-i", "sine=frequency=220:sample_rate=48000:duration=1,aformat=channel_layouts=5.1",
            "-map", "0", "-map", "1", "-map", "2", "-c:a", "pcm_s16le",
        ],
    )
}

#[test]
fn every_audio_stream_is_listed_and_playable() {
    let Some(r) = three_streams() else { return eprintln!("skipped: no ffmpeg CLI") };
    let ff = Ffmpeg::new();
    let info = ff.probe(&r).unwrap();
    let layouts: Vec<(&str, u16)> = info.audio.iter().map(|a| (a.layout.as_str(), a.channels)).collect();
    assert_eq!(layouts, [("mono", 1), ("mono", 1), ("5.1", 6)]);
    for (stream, hz) in [(0, 440.0), (1, 880.0)] {
        let mut d = ff.open_audio(&r, stream, 48_000, 1).unwrap();
        let mut s = Vec::new();
        while let Some(b) = d.next_block().unwrap() {
            s.extend(b.samples);
        }
        let f = crossings(&s, 48_000) / 2.0;
        assert!((f - hz).abs() < 5.0, "stream {stream}: {f} Hz, wanted {hz}");
    }
    // 5.1 downmixed to stereo: sound in both channels.
    let mut d = ff.open_audio(&r, 2, 48_000, 2).unwrap();
    let b = d.next_block().unwrap().unwrap();
    let peak = |c: usize| b.samples.iter().skip(c).step_by(2).fold(0f32, |m, v| m.max(v.abs()));
    assert!(peak(0) > 0.01 && peak(1) > 0.01, "5.1 downmix: L {} R {}", peak(0), peak(1));
    assert!(ff.open_audio(&r, 3, 48_000, 2).is_err(), "no fourth stream");
}
