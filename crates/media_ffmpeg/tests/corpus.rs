//! A corpus of real-world media: the codecs, containers and quirks phones
//! and cameras produce, made by the `ffmpeg` command-line tool (installed on
//! every CI runner; skipped where it is missing or lacks an encoder).
//!
//! Every frame's brightness is its frame number (luma 32 + 4·(n mod 40)), so
//! a decode or a seek can be checked for landing on exactly the right frame,
//! through lossy codecs.

use std::path::{Path, PathBuf};
use std::process::Command;

use media_ffmpeg::Ffmpeg;
use ve_ports::{MediaBackend, PixelFormat, Resolved, VideoFrame};
use ve_time::{Rate, Time};

/// The frame-number pattern, as an ffmpeg filter on a flat input.
const NUMBERED: &str = "geq=lum='32+4*mod(N\\,40)':cb=128:cr=128";

fn dir() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("corpus");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn ffmpeg(args: &[&str]) -> bool {
    Command::new("ffmpeg").args(["-y", "-loglevel", "error"]).args(args).status().is_ok_and(|s| s.success())
}

fn has_encoder(name: &str) -> bool {
    Command::new("ffmpeg")
        .args(["-hide_banner", "-encoders"])
        .output()
        .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.split_whitespace().nth(1) == Some(name)))
}

/// `size` numbered frames at `rate`, `seconds` long, through `encode`.
fn numbered(name: &str, size: &str, rate: &str, seconds: f64, encode: &[&str]) -> Option<PathBuf> {
    let out = dir().join(name);
    let src = format!("color=c=black:size={size}:rate={rate}:duration={seconds},format=yuv420p,{NUMBERED}");
    ffmpeg(&[&["-f", "lavfi", "-i", &src], encode, &[out.to_str()?]].concat()).then_some(out)
}

fn res(p: &Path) -> Resolved {
    Resolved { path: Some(p.to_path_buf()), guard: Box::new(()) }
}

/// The mean luma of a frame's centre, as an 8-bit code value.
fn luma(f: &VideoFrame) -> f64 {
    let ve_ports::CpuPlanes { planes, strides } = f.data.cpu().expect("a CPU frame");
    let (w, h) = (f.width as usize, f.height as usize);
    let (mut sum, mut n) = (0f64, 0f64);
    for y in h / 3..2 * h / 3 {
        for x in w / 3..2 * w / 3 {
            let v = match f.format {
                PixelFormat::Nv12 | PixelFormat::Yuv420p => planes[0][y * strides[0] + x] as f64,
                // P010: the top 10 bits of 16. 4:2:2 10-bit: the low 10 bits.
                PixelFormat::P010 => u16::from_le_bytes([planes[0][y * strides[0] + 2 * x], planes[0][y * strides[0] + 2 * x + 1]]) as f64 / 256.0,
                PixelFormat::Yuv422p10 => u16::from_le_bytes([planes[0][y * strides[0] + 2 * x], planes[0][y * strides[0] + 2 * x + 1]]) as f64 / 4.0,
                other => panic!("unexpected format {other:?}"),
            };
            sum += v;
            n += 1.0;
        }
    }
    sum / n
}

/// The frame number a frame shows (mod 40).
fn number(f: &VideoFrame) -> i64 {
    ((luma(f) - 32.0) / 4.0).round() as i64
}

/// The display size of `info`'s video: rotation and pixel aspect applied.
fn display(path: &Path) -> (u32, u32) {
    let v = Ffmpeg::new().probe(&res(path)).unwrap().video.unwrap();
    (v.display_width(), v.display_height())
}

/// Seek to frame `k` and check the decoder lands on exactly it, for a few
/// frames spread through the clip (keyframes and not).
fn frame_exact(path: &Path, rate: Rate, ks: &[i64]) {
    let mut d = Ffmpeg::new().open_video(&res(path)).unwrap();
    for &k in ks {
        let t = rate.frame_to_time(k);
        d.seek(t).unwrap();
        let f = d.next_frame().unwrap().unwrap_or_else(|| panic!("{}: nothing at frame {k}", path.display()));
        assert_eq!(f.pts, t, "{}: seek to frame {k} gave pts {:?}", path.display(), f.pts);
        assert_eq!(number(&f), k % 40, "{}: seek to frame {k} shows frame {}", path.display(), number(&f));
    }
}

macro_rules! need {
    ($e:expr, $why:expr) => {
        match $e {
            Some(v) => v,
            None => return eprintln!("skipped: {}", $why),
        }
    };
}

#[test]
fn h264_ntsc_rate_is_frame_exact() {
    let p = need!(
        numbered("h264_2997.mp4", "640x360", "30000/1001", 4.0, &["-c:v", "libx264", "-crf", "12", "-g", "48", "-bf", "2", "-pix_fmt", "yuv420p"]),
        "no libx264"
    );
    let info = Ffmpeg::new().probe(&res(&p)).unwrap();
    let v = info.video.unwrap();
    assert_eq!((v.width, v.height, v.rate), (640, 360, Rate::FPS_29_97));
    frame_exact(&p, Rate::FPS_29_97, &[0, 1, 47, 48, 49, 100, 119]);
}

/// An iPhone video held upright: HEVC 10-bit HLG, stored landscape with a
/// 90° display rotation. It must come out portrait.
#[test]
fn iphone_portrait_hevc_hlg() {
    if !has_encoder("libx265") {
        return eprintln!("skipped: no libx265");
    }
    let land = need!(
        numbered("hevc_land.mov", "640x360", "30", 2.0, &[
            "-vf", "setparams=color_primaries=bt2020:color_trc=arib-std-b67:colorspace=bt2020nc",
            "-c:v", "libx265", "-x265-params", "log-level=error:crf=12", "-pix_fmt", "yuv420p10le", "-tag:v", "hvc1",
        ]),
        "hevc encode failed"
    );
    let p = dir().join("iphone_portrait.mov");
    assert!(ffmpeg(&["-display_rotation:v:0", "90", "-i", land.to_str().unwrap(), "-c", "copy", p.to_str().unwrap()]));
    assert_eq!(display(&p), (360, 640), "portrait, as the phone was held");
    let mut d = Ffmpeg::new().open_video(&res(&p)).unwrap();
    let f = d.next_frame().unwrap().unwrap();
    assert_eq!(f.format, PixelFormat::P010, "10-bit stays 10-bit");
    assert_eq!((f.color.primaries.as_str(), f.color.transfer.as_str()), ("bt2020", "arib-std-b67"));
    frame_exact(&p, Rate::new(30, 1), &[0, 31, 59]);
}

#[test]
fn prores_422hq_with_24_bit_pcm() {
    if !has_encoder("prores_ks") {
        return eprintln!("skipped: no prores_ks");
    }
    let p = dir().join("prores.mov");
    let src = format!("color=c=black:size=640x360:rate=24000/1001:duration=2,format=yuv420p,{NUMBERED}");
    assert!(ffmpeg(&[
        "-f", "lavfi", "-i", &src, "-f", "lavfi", "-i", "sine=frequency=1000:sample_rate=48000:duration=2",
        "-c:v", "prores_ks", "-profile:v", "3", "-pix_fmt", "yuv422p10le", "-c:a", "pcm_s24le", "-ac", "2", p.to_str().unwrap(),
    ]));
    let info = Ffmpeg::new().probe(&res(&p)).unwrap();
    assert_eq!(info.video.as_ref().unwrap().rate, Rate::FPS_23_976);
    assert_eq!((info.audio[0].sample_rate, info.audio[0].channels), (48_000, 2));
    frame_exact(&p, Rate::FPS_23_976, &[0, 13, 40]);
}

#[test]
fn vp9_webm() {
    let p = need!(numbered("vp9.webm", "640x360", "25", 3.0, &["-c:v", "libvpx-vp9", "-crf", "15", "-b:v", "0", "-g", "30", "-deadline", "realtime"]), "no libvpx-vp9");
    assert_eq!(display(&p), (640, 360));
    frame_exact(&p, Rate::FPS_25, &[0, 29, 30, 31, 70]);
}

#[test]
fn av1_mkv() {
    if !has_encoder("libsvtav1") {
        return eprintln!("skipped: no libsvtav1");
    }
    let p = need!(numbered("av1.mkv", "640x360", "25", 2.0, &["-c:v", "libsvtav1", "-crf", "20", "-g", "25"]), "av1 encode failed");
    frame_exact(&p, Rate::FPS_25, &[0, 24, 25, 40]);
}

/// DV-style anamorphic: 720×480 stored, 16:9 shown (pixels 32:27 wide).
#[test]
fn anamorphic_pixels_are_shown_wide() {
    let p = need!(numbered("anamorphic.mp4", "720x480", "30000/1001", 1.0, &["-c:v", "libx264", "-vf", "setsar=32/27", "-pix_fmt", "yuv420p"]), "no libx264");
    assert_eq!(display(&p), (853, 480));
}

/// Phones record variable frame rate: every frame must come out at its own
/// time (what the container says), not at a constant rate's guess.
#[test]
fn variable_frame_rate_keeps_each_frames_time() {
    // Frame n at n/30 s, plus a half-frame hitch every third frame.
    let p = need!(
        numbered("vfr.mp4", "320x180", "30", 2.0, &["-vf", "settb=1/30000,setpts='(N+0.5*floor(N/3))/30/TB'", "-fps_mode", "passthrough", "-video_track_timescale", "30000", "-c:v", "libx264", "-crf", "12", "-pix_fmt", "yuv420p"]),
        "no libx264"
    );
    // The container's own timestamps, as ffprobe reads them.
    let out = Command::new("ffprobe").args(["-v", "error", "-select_streams", "v", "-show_entries", "frame=pts_time", "-of", "csv=p=0"]).arg(&p).output();
    let Ok(out) = out else { return eprintln!("skipped: no ffprobe") };
    let want: Vec<f64> = String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim_end_matches(',').parse().ok()).collect();
    let gaps: Vec<f64> = want.windows(2).map(|w| ((w[1] - w[0]) * 1000.0).round()).collect();
    assert!(gaps.iter().any(|&g| g != gaps[0]), "the file really is variable rate: {gaps:?}");
    let mut d = Ffmpeg::new().open_video(&res(&p)).unwrap();
    let mut n = 0usize;
    while let Some(f) = d.next_frame().unwrap() {
        assert!((f.pts.as_seconds_f64() - want[n]).abs() < 0.001, "frame {n}: pts {} s, the file says {}", f.pts.as_seconds_f64(), want[n]);
        assert_eq!(number(&f), n as i64 % 40, "frame {n} out of order");
        n += 1;
    }
    assert_eq!(n, want.len());
}

/// A camera file with a 5.1 mix and a separate stereo stream.
#[test]
fn surround_and_a_second_audio_stream() {
    let p = dir().join("two_streams.mov");
    assert!(ffmpeg(&[
        "-f", "lavfi", "-i", "color=c=gray:size=320x180:rate=25:duration=2",
        "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2",
        "-f", "lavfi", "-i", "sine=frequency=880:sample_rate=48000:duration=2",
        "-filter_complex", "[1]pan=5.1|FL=c0|FR=c0|FC=c0|LFE=c0|BL=c0|BR=c0[s]",
        "-map", "0:v", "-map", "[s]", "-map", "2:a", "-c:v", "mpeg4", "-c:a", "aac", "-ac:a:1", "2", p.to_str().unwrap(),
    ]));
    let info = Ffmpeg::new().probe(&res(&p)).unwrap();
    let got: Vec<(u16, &str)> = info.audio.iter().map(|a| (a.channels, a.layout.as_str())).collect();
    assert_eq!(got, [(6, "5.1"), (2, "stereo")]);
    // Each stream decodes on its own, folded to what the mixer asks for.
    for s in 0..2 {
        let mut d = Ffmpeg::new().open_audio(&res(&p), s, 48_000, 2).unwrap();
        let b = d.next_block().unwrap().unwrap();
        assert_eq!((b.sample_rate, b.channels), (48_000, 2));
    }
}

#[test]
fn high_resolution_wav() {
    let p = dir().join("hires.wav");
    assert!(ffmpeg(&["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=96000:duration=1", "-c:a", "pcm_s24le", p.to_str().unwrap()]));
    let info = Ffmpeg::new().probe(&res(&p)).unwrap();
    assert!(info.video.is_none());
    assert_eq!(info.audio[0].sample_rate, 96_000);
    let mut d = Ffmpeg::new().open_audio(&res(&p), 0, 48_000, 2).unwrap();
    let mut total = 0;
    while let Some(b) = d.next_block().unwrap() {
        total += b.samples.len() / 2;
    }
    assert!((47_000..=49_000).contains(&total), "one second at 48 kHz: {total}");
}

/// Sizes no codec likes: not a multiple of 16 (or 4).
#[test]
fn awkward_sizes() {
    let p = need!(numbered("odd.mp4", "642x362", "25", 1.0, &["-c:v", "libx264", "-pix_fmt", "yuv420p"]), "no libx264");
    let mut d = Ffmpeg::new().open_video(&res(&p)).unwrap();
    let f = d.next_frame().unwrap().unwrap();
    assert_eq!((f.width, f.height), (642, 362));
    assert_eq!(number(&f), 0);
}

/// UHD: big frames decode whole.
#[test]
fn uhd_frames() {
    let p = need!(numbered("uhd.mp4", "3840x2160", "25", 0.2, &["-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p"]), "no libx264");
    let mut d = Ffmpeg::new().open_video(&res(&p)).unwrap();
    let f = d.next_frame().unwrap().unwrap();
    assert_eq!((f.width, f.height), (3840, 2160));
    d.seek(Rate::FPS_25.frame_to_time(3)).unwrap();
    assert_eq!(number(&d.next_frame().unwrap().unwrap()), 3);
}

/// MP3: the common audio file nobody thinks about.
#[test]
fn mp3() {
    if !has_encoder("libmp3lame") {
        return eprintln!("skipped: no libmp3lame");
    }
    let p = dir().join("song.mp3");
    assert!(ffmpeg(&["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=44100:duration=2", "-c:a", "libmp3lame", "-b:a", "192k", p.to_str().unwrap()]));
    let mut d = Ffmpeg::new().open_audio(&res(&p), 0, 48_000, 2).unwrap();
    d.seek(Time::from_seconds(1)).unwrap();
    let b = d.next_block().unwrap().unwrap();
    assert!((b.pts.as_seconds_f64() - 1.0).abs() < 0.03, "{:?}", b.pts);
}
