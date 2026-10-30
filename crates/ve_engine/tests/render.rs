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
    // CI runners whose software GPU crashes (Windows ARM64) skip GPU tests.
    if std::env::var_os("VE_NO_GPU").is_some() {
        return eprintln!("skipped: VE_NO_GPU");
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { force_fallback_adapter: std::env::var_os("VE_GPU_FALLBACK").is_some(), ..Default::default() })) else { return eprintln!("skipped: no GPU") };
    // Which GPU the tests got (CI logs: a software adapter where there is no GPU).
    eprintln!("test GPU: {:?}", adapter.get_info());
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
    // CI runners whose software GPU crashes (Windows ARM64) skip GPU tests.
    if std::env::var_os("VE_NO_GPU").is_some() {
        return eprintln!("skipped: VE_NO_GPU");
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { force_fallback_adapter: std::env::var_os("VE_GPU_FALLBACK").is_some(), ..Default::default() })) else { return eprintln!("skipped: no GPU") };
    // Which GPU the tests got (CI logs: a software adapter where there is no GPU).
    eprintln!("test GPU: {:?}", adapter.get_info());
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
    assert_eq!(info.audio[0].codec, "aac");
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
    let mut ad = ff.open_audio(&r, 0, 48_000, 2).unwrap();
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
    // CI runners whose software GPU crashes (Windows ARM64) skip GPU tests.
    if std::env::var_os("VE_NO_GPU").is_some() {
        return eprintln!("skipped: VE_NO_GPU");
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { force_fallback_adapter: std::env::var_os("VE_GPU_FALLBACK").is_some(), ..Default::default() })) else { return eprintln!("skipped: no GPU") };
    // Which GPU the tests got (CI logs: a software adapter where there is no GPU).
    eprintln!("test GPU: {:?}", adapter.get_info());
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
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            assert!(std::time::Instant::now() < deadline, "export hung at {:.0}%", job.progress() * 100.0);
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
    // The file goes away. (Windows will not delete a file the engine has
    // open, so the asset is pointed at where it no longer is instead: the
    // same missing media to an export.)
    let gone_path = dir.join("will_vanish_moved_away.mov");
    let info = client.snapshot().assets[&gone].info.clone();
    client.execute(ve_engine::Command::SetAssetMedia { asset: gone, media: MediaRef(format!("file:{}", gone_path.display())), info });
    let moved = MediaRef(format!("file:{}", gone_path.display()));
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while client.snapshot().assets[&gone].media != moved {
        assert!(std::time::Instant::now() < deadline, "relink never landed");
        std::thread::sleep(Duration::from_millis(5));
    }
    let err = run(gpu, "bad.mov").unwrap_err();
    assert!(err.contains("will_vanish.mov"), "{err}");
    assert!(!dir.join("bad.mov").exists(), "no half-written file");
}

/// A 10-bit gradient exported to ProRes keeps far more than 256 levels: the
/// export no longer goes through 8 bits.
#[test]
fn prores_export_keeps_ten_bits() {
    let src = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("grad10.mov");
    let made = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "nullsrc=s=1024x64:d=0.2,format=gray16le,geq=lum='X*64'"])
        .args(["-pix_fmt", "yuv422p10le", "-c:v", "prores_ks", "-profile:v", "3"])
        .arg(&src)
        .status()
        .is_ok_and(|s| s.success());
    if !made {
        return eprintln!("skipped: no ffmpeg CLI");
    }
    // CI runners whose software GPU crashes (Windows ARM64) skip GPU tests.
    if std::env::var_os("VE_NO_GPU").is_some() {
        return eprintln!("skipped: VE_NO_GPU");
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { force_fallback_adapter: std::env::var_os("VE_GPU_FALLBACK").is_some(), ..Default::default() })) else { return eprintln!("skipped: no GPU") };
    // Which GPU the tests got (CI logs: a software adapter where there is no GPU).
    eprintln!("test GPU: {:?}", adapter.get_info());
    if !adapter.features().contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM) {
        return eprintln!("skipped: no 16-bit textures (10-bit sources are read at 8 bits)");
    }
    let desc = wgpu::DeviceDescriptor { required_features: wgpu::Features::TEXTURE_FORMAT_16BIT_NORM, ..Default::default() };
    let gpu = pollster::block_on(adapter.request_device(&desc)).unwrap();

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("export10");
    let mut e = Engine::new(platform_headless::platform(&dir, Arc::new(media_ffmpeg::Ffmpeg::new())));
    e.new_project("t");
    let asset = e.import(src.to_str().unwrap()).unwrap();
    let snap = e.snapshot();
    let seq = snap.active().unwrap().clone();
    let a = snap.assets[&asset].clone();
    let clip = make_clip(e.plugins(), &seq.format, &a, TrackKind::Video, Time::from_ticks(ve_time::TICKS_PER_SECOND / 10), None);
    e.execute(edit::overwrite(&snap, seq.id, Time::ZERO, &[(seq.tracks[0].id, Arc::new(clip))]).unwrap()).unwrap();
    let out = dir.join("grad.mov");
    let client = e.spawn(Arc::new(|| {}));
    let job = client.export(gpu, ExportPreset::ProResMov, MediaRef(format!("file:{}", out.display())), None);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let result = loop {
        if let Some(r) = job.result() {
            break r;
        }
        assert!(std::time::Instant::now() < deadline, "export hung");
        std::thread::sleep(Duration::from_millis(20));
    };
    result.unwrap();

    use ve_ports::{MediaBackend, Resolved};
    let mut d = media_ffmpeg::Ffmpeg::new().open_video(&Resolved { path: Some(out), guard: Box::new(()) }).unwrap();
    let f = d.next_frame().unwrap().unwrap();
    assert_eq!(f.format, ve_ports::PixelFormat::P010, "a 10-bit file");
    let ve_ports::CpuPlanes { planes, strides } = f.data.cpu().unwrap();
    let row = &planes[0][540 * strides[0]..540 * strides[0] + 1920 * 2];
    let mut levels: Vec<u16> = row.chunks(2).map(|b| u16::from_le_bytes([b[0], b[1]]) >> 6).collect();
    levels.sort();
    levels.dedup();
    eprintln!("{} distinct 10-bit luma levels across the exported gradient", levels.len());
    assert!(levels.len() > 600, "{} distinct luma levels across the gradient (8-bit export gives ~220)", levels.len());
}

/// Create Proxies on a 1280x720 clip: a 540p H.264 file appears under
/// Documents/Proxies, is attached to the asset as its proxy, matches the
/// original's length and rate, and shows the same picture.
#[test]
fn create_proxies_makes_and_attaches_a_small_copy() {
    use ve_ports::{MediaBackend, Resolved};
    let Some(src) = generated("proxy_src.mov", &["-f", "lavfi", "-i", "smptebars=size=1280x720:rate=25:duration=2", "-c:v", "mpeg4", "-q:v", "2"]) else {
        return eprintln!("skipped: no ffmpeg CLI");
    };
    // CI runners whose software GPU crashes (Windows ARM64) skip GPU tests.
    if std::env::var_os("VE_NO_GPU").is_some() {
        return eprintln!("skipped: VE_NO_GPU");
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { force_fallback_adapter: std::env::var_os("VE_GPU_FALLBACK").is_some(), ..Default::default() })) else { return eprintln!("skipped: no GPU") };
    // Which GPU the tests got (CI logs: a software adapter where there is no GPU).
    eprintln!("test GPU: {:?}", adapter.get_info());
    let gpu = pollster::block_on(adapter.request_device(&Default::default())).unwrap();

    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("proxies");
    let _ = std::fs::remove_dir_all(&dir);
    let mut e = Engine::new(platform_headless::platform(&dir, Arc::new(media_ffmpeg::Ffmpeg::new())));
    e.new_project("t");
    let asset = e.import(src.to_str().unwrap()).unwrap();
    let client = e.spawn(Arc::new(|| {}));
    client.enable_jobs(gpu, None);
    let ids = client.queue_proxies(vec![asset], 540);
    assert_eq!(ids.len(), 1);
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    client.jobs().wait_idle();
    let job = client.jobs().list().into_iter().find(|j| j.id == ids[0]).unwrap();
    assert_eq!((job.kind, job.title.as_str()), ("Proxy", "proxy_src.mov"));
    assert_eq!(job.state, JobState::Done, "{:?}", job.state);
    let path = job.detail.clone();
    assert!(path.ends_with("Proxies/proxy_src Proxy.mp4"), "{path}");
    // Attached, once the engine has probed it.
    let proxy = loop {
        let snap = client.snapshot();
        if let Some(v) = snap.assets[&asset].variants.iter().find(|v| v.kind == VariantKind::Proxy) {
            break v.clone();
        }
        assert!(std::time::Instant::now() < deadline, "the proxy was never attached");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!((proxy.width, proxy.height), (960, 540));
    let info = media_ffmpeg::Ffmpeg::new().probe(&Resolved { path: Some(path.clone().into()), guard: Box::new(()) }).unwrap();
    let v = info.video.unwrap();
    assert_eq!((v.width, v.height), (960, 540));
    assert!(["h264", "mpeg4"].contains(&v.codec.as_str()), "H.264, or MPEG-4 where no H.264 encoder exists: {}", v.codec);
    assert_eq!(v.rate, ve_time::Rate::FPS_25);
    let secs = info.duration.as_seconds_f64();
    assert!((secs - 2.0).abs() < 0.1, "as long as the original: {secs}");
}

/// The job queue with real exports: one at a time in order; held while the
/// editor plays and carried on after; a cancelled one leaves no file; and
/// when the queue is empty its thread is gone (race to idle).
#[test]
fn the_job_queue_runs_holds_and_cancels_exports() {
    let Some(src) = clip() else { return eprintln!("skipped: no ffmpeg CLI") };
    if std::env::var_os("VE_NO_GPU").is_some() {
        return eprintln!("skipped: VE_NO_GPU");
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let Ok(adapter) = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { force_fallback_adapter: std::env::var_os("VE_GPU_FALLBACK").is_some(), ..Default::default() })) else { return eprintln!("skipped: no GPU") };
    let gpu = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("jobs");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = Engine::new(platform_headless::platform(&dir, Arc::new(media_ffmpeg::Ffmpeg::new())));
    e.new_project("t");
    let asset = e.import(src.to_str().unwrap()).unwrap();
    let snap = e.snapshot();
    let seq = snap.active().unwrap().clone();
    let a = snap.assets[&asset].clone();
    let c = make_clip(e.plugins(), &seq.format, &a, TrackKind::Video, Time::from_seconds(2), None);
    e.execute(edit::overwrite(&snap, seq.id, Time::ZERO, &[(seq.tracks[0].id, Arc::new(c))]).unwrap()).unwrap();
    let client = e.spawn(Arc::new(|| {}));
    let out = |n: &str| MediaRef(format!("file:{}", dir.join(n).display()));
    // Queued before the GPU is ready: they wait.
    let first = client.queue_export(ExportPreset::ProResMov, out("first.mov"));
    let second = client.queue_export(ExportPreset::ProResMov, out("second.mov"));
    let third = client.queue_export(ExportPreset::ProResMov, out("third.mov"));
    let state = |id| client.jobs().list().into_iter().find(|j| j.id == id).unwrap().state;
    assert_eq!(state(first), JobState::Queued);
    // The editor is playing when the GPU arrives: the first starts, held.
    // (The engine's transport tells the queue; nothing else has to.)
    client.play(1.0);
    client.jobs().cancel(third);
    client.enable_jobs(gpu, None);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while state(first) != JobState::Held {
        assert!(std::time::Instant::now() < deadline, "the first export never started: {:?}", state(first));
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(state(first), JobState::Held, "held while playing");
    assert_eq!(state(second), JobState::Queued, "one at a time");
    assert_eq!(client.jobs().list().iter().find(|j| j.id == first).unwrap().progress, 0.0, "held before its first frame");
    client.stop();
    client.jobs().wait_idle();
    assert_eq!((state(first), state(second), state(third)), (JobState::Done, JobState::Done, JobState::Cancelled));
    assert!(dir.join("first.mov").exists() && dir.join("second.mov").exists());
    assert!(!dir.join("third.mov").exists(), "a job cancelled before it ran made nothing");
    // Nothing left: the runner thread has ended.
    assert!(!client.jobs().active());
    client.jobs().clear_finished();
    assert!(client.jobs().list().is_empty());
}

/// An engine with a 2 s clip on the timeline, for the job queue tests below.
fn queue_setup(dir_name: &str) -> Option<(EngineClient, PathBuf, (wgpu::Device, wgpu::Queue))> {
    let src = clip()?;
    if std::env::var_os("VE_NO_GPU").is_some() {
        return None;
    }
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { force_fallback_adapter: std::env::var_os("VE_GPU_FALLBACK").is_some(), ..Default::default() })).ok()?;
    let gpu = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(dir_name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut e = Engine::new(platform_headless::platform(&dir, Arc::new(media_ffmpeg::Ffmpeg::new())));
    e.new_project("t");
    let asset = e.import(src.to_str().unwrap()).unwrap();
    let snap = e.snapshot();
    let seq = snap.active().unwrap().clone();
    let a = snap.assets[&asset].clone();
    let c = make_clip(e.plugins(), &seq.format, &a, TrackKind::Video, Time::from_seconds(2), None);
    e.execute(edit::overwrite(&snap, seq.id, Time::ZERO, &[(seq.tracks[0].id, Arc::new(c))]).unwrap()).unwrap();
    Some((e.spawn(Arc::new(|| {})), dir, gpu))
}

/// Pausing a job part-way gives the next one its turn; resumed, it waits
/// for its turn and carries on from where it stopped, and its file is whole.
/// `wait_idle` returns once only paused jobs are left.
#[test]
fn a_paused_job_yields_and_resumes_where_it_stopped() {
    let Some((client, dir, gpu)) = queue_setup("jobs_pause") else { return eprintln!("skipped: no ffmpeg CLI or GPU") };
    let out = |n: &str| MediaRef(format!("file:{}", dir.join(n).display()));
    let state = |id| client.jobs().list().into_iter().find(|j| j.id == id).unwrap();
    // Playing: the first starts held, before its first frame.
    client.play(1.0);
    let a = client.queue_export(ExportPreset::ProResMov, out("a.mov"));
    let b = client.queue_export(ExportPreset::ProResMov, out("b.mov"));
    client.enable_jobs(gpu, None);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while state(a).state != JobState::Held {
        assert!(std::time::Instant::now() < deadline, "the first export never started: {:?}", state(a).state);
        std::thread::sleep(Duration::from_millis(5));
    }
    // Paused: the second takes its turn and finishes.
    client.jobs().pause(a, true);
    client.stop();
    client.jobs().wait_idle();
    assert_eq!(state(a).state, JobState::Paused);
    assert_eq!(state(b).state, JobState::Done, "the queue went on past the paused job");
    assert_eq!(state(a).progress, 0.0, "paused before its first frame, it made none");
    // Resumed: it finishes, as long as the other.
    client.jobs().pause(a, false);
    client.jobs().wait_idle();
    assert_eq!(state(a).state, JobState::Done);
    let probe = |n: &str| {
        use ve_ports::MediaBackend;
        let r = ve_ports::Resolved { path: Some(dir.join(n)), guard: Box::new(()) };
        media_ffmpeg::Ffmpeg::new().probe(&r).unwrap().duration.as_seconds_f64()
    };
    assert!((probe("a.mov") - probe("b.mov")).abs() < 0.05, "a: {} s, b: {} s", probe("a.mov"), probe("b.mov"));
    assert!(state(a).ran.is_some() && !client.jobs().active());
}

/// Cancelled the moment it starts: never Done, and nothing left on disk
/// (it was once marked Cancelled and then overwritten by Done when the
/// export, started anyway, finished).
#[test]
fn a_job_cancelled_as_it_starts_makes_nothing() {
    let Some((client, dir, gpu)) = queue_setup("jobs_cancel") else { return eprintln!("skipped: no ffmpeg CLI or GPU") };
    client.enable_jobs(gpu, None);
    for n in 0..5 {
        let name = format!("c{n}.mov");
        let id = client.queue_export(ExportPreset::ProResMov, MediaRef(format!("file:{}", dir.join(&name).display())));
        client.jobs().cancel(id);
        client.jobs().wait_idle();
        let s = client.jobs().list().into_iter().find(|j| j.id == id).unwrap().state;
        assert_eq!(s, JobState::Cancelled);
        assert!(!dir.join(&name).exists(), "a cancelled export left {name}");
    }
}
