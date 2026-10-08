//! Hardware-decoded frames imported without a copy must look exactly like the
//! same frames decoded to memory. Needs the `ffmpeg` CLI, a GPU, and (for
//! anything to be imported) Apple's VideoToolbox; skips otherwise.

use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;

use media_ffmpeg::VideoDec;
use ve_ports::{FrameData, Resolved, VideoDecoder, VideoFrame};
use ve_render::*;
use ve_time::Time;

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
    let features = adapter.features() & wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor { required_features: features, ..Default::default() })).ok()
}

fn clip(name: &str, args: &[&str]) -> Option<Resolved> {
    let p = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let ok = Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", "smptebars=size=640x360:rate=25:duration=0.4"])
        .args(args)
        .arg(&p)
        .status()
        .is_ok_and(|s| s.success());
    ok.then(|| Resolved { path: Some(p), guard: Box::new(()) })
}

fn first(r: &Resolved, native: bool) -> Arc<VideoFrame> {
    let mut d = VideoDec::open_with(r, true, native).unwrap();
    Arc::new(d.next_frame().unwrap().unwrap())
}

fn render(dev: &(wgpu::Device, wgpu::Queue), frame: Arc<VideoFrame>, importer: Option<Arc<dyn TextureImporter>>) -> Vec<u8> {
    let (device, queue) = dev;
    let mut c = Compositor::new(device);
    c.set_importer(importer);
    let (w, h) = (frame.width, frame.height);
    let layer = RenderLayer {
        motion: Motion { position: [w as f32 / 2.0, h as f32 / 2.0], scale: 100.0, rotation: 0.0, anchor: [w as f32 / 2.0, h as f32 / 2.0], crop: [0.0; 4] },
        frame,
        opacity: 1.0,
        blend: Blend::Normal,
        effects: vec![],
    };
    let plan = FramePlan { time: Time::ZERO, width: w, height: h, layers: vec![] };
    c.render(device, queue, &plan, &[layer], (w, h), WorkingSpace::AcesCg);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.read_output(device, queue).unwrap().2
}

fn check(name: &str, args: &[&str]) {
    let Some(r) = clip(name, args) else { return eprintln!("skipped: no ffmpeg CLI / encoder for {name}") };
    let Some(dev) = gpu() else { return eprintln!("skipped: no GPU") };
    let native = first(&r, true);
    if !matches!(native.data, FrameData::Native(_)) {
        return eprintln!("skipped: {name} was not hardware-decoded here");
    }
    // The importer really takes it (not the copy fallback, which also matches).
    let [luma, chroma] = gpu_import::importer().expect("an importer here").import(&dev.0, &native).expect("imported");
    let deep = native.format == ve_ports::PixelFormat::P010;
    assert_eq!((luma.format(), chroma.format()), if deep { (wgpu::TextureFormat::R16Unorm, wgpu::TextureFormat::Rg16Unorm) } else { (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm) });
    assert_eq!((luma.width(), chroma.width()), (native.width, native.width / 2));
    let reference = render(&dev, first(&r, false), None);
    let imported = render(&dev, native.clone(), gpu_import::importer());
    let copied = render(&dev, native, None); // no importer: the copy fallback
    for (what, img) in [("imported", &imported), ("copied", &copied)] {
        let worst = img.iter().zip(&reference).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
        assert!(worst <= 1, "{name}: {what} frame differs from memory decode by up to {worst}");
    }
}

#[test]
fn nv12_imports_like_memory() {
    check("import_h264.mp4", &["-c:v", "h264_videotoolbox", "-pix_fmt", "nv12"]);
}

#[test]
fn p010_imports_like_memory() {
    check("import_hevc10.mp4", &["-c:v", "hevc_videotoolbox", "-profile:v", "main10", "-pix_fmt", "p010le", "-tag:v", "hvc1"]);
}
