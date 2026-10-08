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

fn clip() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("render_src.mov");
    if !p.exists() {
        let ok = Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "smptebars=size=640x360:rate=25:duration=3"])
            .args(["-c:v", "mpeg4", "-q:v", "2"])
            .arg(&p)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return None;
        }
    }
    Some(p)
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
    use ve_ports::{FrameData, MediaBackend, Resolved};
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
    let job = client.export(gpu, ExportPreset::H264Mp4, MediaRef(format!("file:{}", out.display())));
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    let result = loop {
        if let Some(r) = job.result() {
            break r;
        }
        assert!(std::time::Instant::now() < deadline, "export hung at {:.0}%", job.progress() * 100.0);
        std::thread::sleep(Duration::from_millis(20));
    };
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
    let FrameData::Cpu { planes, strides } = &f.data else { panic!() };
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
