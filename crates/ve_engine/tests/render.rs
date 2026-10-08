//! Import → timeline → decode → composite → pixels, with real FFmpeg and a
//! real GPU. Skips without the `ffmpeg` CLI or a GPU.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use ve_engine::*;
use ve_model::*;
use ve_render::Compositor;
use ve_time::Time;

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

fn clip() -> Option<PathBuf> {
    generated("render_src.mov", &["-f", "lavfi", "-i", "smptebars=size=640x360:rate=25:duration=3", "-c:v", "mpeg4", "-q:v", "2"])
}

#[test]
fn import_place_decode_composite() {
    let Some(file) = clip() else { return eprintln!("skipped: no ffmpeg CLI") };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else { return eprintln!("skipped: no GPU") };
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("render");
    let mut e = Engine::new(platform_headless::platform(&dir, Arc::new(media_ffmpeg::Ffmpeg::new())));
    e.new_project("t");
    let asset = e.import(file.to_str().unwrap()).unwrap();
    let snap = e.snapshot();
    let seq = snap.active().unwrap().clone();
    let a = snap.assets[&asset].clone();
    let c = make_clip(e.plugins(), &seq.format, &a, TrackKind::Video, Time::from_seconds(3), None);
    e.execute(edit::overwrite(&snap, seq.id, Time::ZERO, &[(seq.tracks[0].id, Arc::new(c))]).unwrap()).unwrap();
    e.seek(Time::from_seconds(1));

    let plan = e.frame_plan(Quality::FULL).unwrap();
    let frame = ve_engine::frame::resolve(&e.snapshot(), plan, e.plugins(), e.video(), Some(Duration::from_secs(5)));
    assert!(frame.complete && frame.layers.len() == 1);
    let mut comp = Compositor::new(&device);
    comp.render(&device, &queue, &frame.plan, &frame.layers, frame.seq_size, frame.space);
    let (w, h, px) = comp.read_output(&device, &queue).unwrap();
    assert_eq!((w, h), (1920, 1080));

    // 640x360 SMPTE bars, scaled to fit the 1080p frame (Motion scale 300%).
    let at = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2]]
    };
    // The first bar (75% white) at the left of the picture.
    let first = at(60, 200);
    assert!(first.iter().all(|c| (180..=200).contains(c)), "75% white bar: {first:?}");
    // The second bar is yellow: red and green high, blue low.
    let yellow = at(60 + 276, 200);
    assert!(yellow[0] > 170 && yellow[1] > 170 && yellow[2] < 30, "yellow bar: {yellow:?}");

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ui-look/monitor.png");
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let f = std::fs::File::create(&out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&px).unwrap();
}

#[test]
fn export_h264_with_audio() {
    use ve_ports::{MediaBackend, Resolved};
    let src = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("export_src.mov");
    let made = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "smptebars=size=640x360:rate=25:duration=2"])
        .args(["-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000:duration=2"])
        .args(["-c:v", "mpeg4", "-q:v", "2", "-c:a", "pcm_s16le", "-shortest"])
        .arg(&src)
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        return eprintln!("skipped: no ffmpeg CLI");
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else { return eprintln!("skipped: no GPU") };
    let gpu = pollster::block_on(adapter.request_device(&Default::default())).unwrap();

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("export");
    let mut e = Engine::new(platform_headless::platform(&dir, Arc::new(media_ffmpeg::Ffmpeg::new())));
    e.new_project("t");
    let asset = e.import(src.to_str().unwrap()).unwrap();
    let snap = e.snapshot();
    let seq = snap.active().unwrap().clone();
    let a = snap.assets[&asset].clone();
    let len = a.info.as_ref().unwrap().duration;
    let link = Some(LinkId::new());
    let items = [
        (seq.tracks[0].id, Arc::new(make_clip(e.plugins(), &seq.format, &a, TrackKind::Video, len, link))),
        (seq.tracks[2].id, Arc::new(make_clip(e.plugins(), &seq.format, &a, TrackKind::Audio, len, link))),
    ];
    e.execute(edit::overwrite(&snap, seq.id, Time::ZERO, &items).unwrap()).unwrap();

    let out = dir.join("out.mp4");
    let client = e.spawn(Arc::new(|| {}));
    let job = client.export(gpu, ExportPreset::H264Mp4, MediaRef(format!("file:{}", out.display())), None);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let result = loop {
        if let Some(r) = job.result() {
            break r;
        }
        assert!(std::time::Instant::now() < deadline, "export hung at {:.0}%", job.progress() * 100.0);
        std::thread::sleep(Duration::from_millis(20));
    };
    if let Err(e) = &result {
        if e.contains("encoder works here") {
            // No GPU media engine on this machine (a CI runner); the export
            // pipeline itself is covered with ProRes by the next test.
            return eprintln!("skipped: {e}");
        }
    }
    result.unwrap();
    assert_eq!(job.total, 48, "2 s at 24 fps");

    let ff = media_ffmpeg::Ffmpeg::new();
    let r = Resolved { path: Some(out.clone()), guard: Box::new(()) };
    let info = ff.probe(&r).unwrap();
    let v = info.video.unwrap();
    assert_eq!((v.width, v.height, v.codec.as_str()), (1920, 1080, "h264"));
    assert_eq!(info.audio.unwrap().codec, "aac");
    let secs = info.duration.as_seconds_f64();
    assert!((1.95..2.1).contains(&secs), "{secs}");
    // The picture survived the round trip: the second bar is still yellow.
    let mut d = ff.open_video(&r).unwrap();
    let f = d.next_frame().unwrap().unwrap();
    let ve_ports::CpuPlanes { planes, strides } = f.data.cpu().unwrap();
    let (x, y) = (60 + 276, 200);
    let luma = planes[0][y * strides[0] + x];
    let cb = planes[1][(y / 2) * strides[1] + (x / 2) * 2];
    assert!(luma > 150 && cb < 80, "yellow: high luma, low Cb ({luma}, {cb})");
    // And the audio is there.
    let mut ad = ff.open_audio(&r, 48_000, 2).unwrap();
    let mut peak = 0f32;
    while let Some(b) = ad.next_block().unwrap() {
        peak = b.samples.iter().fold(peak, |m, s| m.max(s.abs()));
    }
    assert!(peak > 0.05, "audio peak {peak}");
}

/// Media gone by export time: the export fails and says which, instead of
/// writing black frames. And it is not disturbed by the preview reading the
/// same source at the same time.
#[test]
fn export_fails_on_missing_media_and_ignores_the_preview() {
    let Some(file) = clip() else { return eprintln!("skipped: no ffmpeg CLI") };
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&Default::default())) else { return eprintln!("skipped: no GPU") };
    let gpu = pollster::block_on(adapter.request_device(&Default::default())).unwrap();

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("export_missing");
    std::fs::create_dir_all(&dir).unwrap();
    let copy = dir.join("will_vanish.mov");
    std::fs::copy(&file, &copy).unwrap();
    let mut e = Engine::new(platform_headless::platform(&dir, Arc::new(media_ffmpeg::Ffmpeg::new())));
    e.new_project("t");
    let keep = e.import(file.to_str().unwrap()).unwrap();
    let gone = e.import(copy.to_str().unwrap()).unwrap();
    let snap = e.snapshot();
    let seq = snap.active().unwrap().clone();
    let len = Time::from_seconds(2);
    let items = [
        (seq.tracks[0].id, Arc::new(make_clip(e.plugins(), &seq.format, &snap.assets[&keep], TrackKind::Video, len, None))),
        (seq.tracks[1].id, Arc::new(make_clip(e.plugins(), &seq.format, &snap.assets[&gone], TrackKind::Video, len, None))),
    ];
    e.execute(edit::overwrite(&snap, seq.id, Time::ZERO, &items).unwrap()).unwrap();
    let client = e.spawn(Arc::new(|| {}));
    let run = |gpu, name: &str| {
        // ProRes: encoded in software, so this runs on any machine.
        let job = client.export(gpu, ExportPreset::ProResMov, MediaRef(format!("file:{}", dir.join(name).display())), None);
        loop {
            // The preview keeps asking for the frame at the playhead.
            client.seek(Time::from_seconds(1));
            let _ = client.frame(Quality::FULL);
            if let Some(r) = job.result() {
                break r;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    run(gpu.clone(), "ok.mov").unwrap();
    std::fs::remove_file(&copy).unwrap();
    let err = run(gpu, "bad.mov").unwrap_err();
    assert!(err.contains("will_vanish.mov"), "{err}");
    assert!(!dir.join("bad.mov").exists(), "no half-written file");
}
