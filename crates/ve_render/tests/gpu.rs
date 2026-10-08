//! The compositor on a real GPU, headless. Skips when no adapter exists (CI
//! without a GPU).

use std::sync::Arc;
use ve_ports::{ColorTags, FrameData, PixelFormat, VideoFrame};
use ve_render::*;
use ve_time::Time;

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    let features = adapter.features() & wgpu::Features::TEXTURE_FORMAT_16BIT_NORM;
    let desc = wgpu::DeviceDescriptor { required_features: features, ..Default::default() };
    pollster::block_on(adapter.request_device(&desc)).ok()
}

/// A flat NV12 frame: limited-range luma `y`, neutral chroma.
fn gray(w: u32, h: u32, y: u8) -> Arc<VideoFrame> {
    Arc::new(VideoFrame {
        pts: Time::ZERO,
        duration: Time::from_seconds(1),
        width: w,
        height: h,
        format: PixelFormat::Nv12,
        color: ColorTags { primaries: "bt709".into(), transfer: "bt709".into(), matrix: "bt709".into(), full_range: false },
        data: FrameData::Cpu { planes: vec![vec![y; (w * h) as usize], vec![128; (w * h / 2) as usize]], strides: vec![w as usize, w as usize] },
    })
}

fn plan(w: u32, h: u32) -> FramePlan {
    FramePlan { time: Time::ZERO, width: w, height: h, layers: vec![] }
}

fn layer(frame: Arc<VideoFrame>, scale: f32, opacity: f32) -> RenderLayer {
    let (w, h) = (frame.width as f32, frame.height as f32);
    RenderLayer {
        frame,
        motion: Motion { position: [32.0, 18.0], scale, rotation: 0.0, anchor: [w / 2.0, h / 2.0], crop: [0.0; 4] },
        opacity,
        blend: Blend::Normal,
        effects: vec![],
    }
}

fn px(img: &(u32, u32, Vec<u8>), x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.0 + x) * 4) as usize;
    [img.2[i], img.2[i + 1], img.2[i + 2], img.2[i + 3]]
}

fn render(layers: &[RenderLayer], space: WorkingSpace) -> Option<(u32, u32, Vec<u8>)> {
    let (device, queue) = gpu()?;
    let mut c = Compositor::new(&device);
    c.render(&device, &queue, &plan(64, 36), layers, (64, 36), space);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    c.read_output(&device, &queue)
}

#[test]
fn rec709_round_trips_through_acescg() {
    let Some(img) = render(&[layer(gray(64, 36, 126), 100.0, 1.0)], WorkingSpace::AcesCg) else {
        return eprintln!("skipped: no GPU");
    };
    // Luma 126 (limited) is 0.502 encoded; it must come back as 128 ± 2.
    let p = px(&img, 32, 18);
    for c in &p[..3] {
        assert!((126..=130).contains(c), "{p:?}");
    }
}

#[test]
fn motion_scale_letterboxes_and_opacity_blends() {
    let Some(img) = render(&[layer(gray(64, 36, 235), 50.0, 0.5)], WorkingSpace::LinearRec709) else {
        return eprintln!("skipped: no GPU");
    };
    // Half size: the corner is black, the centre is the layer.
    assert_eq!(&px(&img, 2, 2)[..3], &[0, 0, 0]);
    // White at 50% over black: linear 0.5 → 0.5^(1/2.4) ≈ 0.749 → 191.
    let c = px(&img, 32, 18)[0];
    assert!((186..=196).contains(&c), "{c}");
}

#[test]
fn plugin_wgsl_runs_as_an_effect() {
    let Some((device, queue)) = gpu() else { return eprintln!("skipped: no GPU") };
    // The SDK example, loaded the way the engine loads it.
    let lib = ve_builtins::linked()[0]();
    let name = lib.name().to_string();
    let fx = ve_plugin_host::native::load(Arc::from(lib), Some(&name)).unwrap().remove(0);
    let mut l = layer(gray(64, 36, 16), 100.0, 1.0); // black…
    l.effects.push(GpuEffect { key: fx.plugin.id.clone(), wgsl: fx.wgsl.unwrap().into(), params: vec![[1.0, 0.0, 0.0, 0.0]], time: 0.0 });
    let mut c = Compositor::new(&device);
    c.render(&device, &queue, &plan(64, 36), &[l], (64, 36), WorkingSpace::LinearRec709);
    assert!(c.errors.is_empty(), "{:?}", c.errors);
    let img = c.read_output(&device, &queue).unwrap();
    // …inverted to white.
    assert!(px(&img, 32, 18)[0] > 250, "{:?}", px(&img, 32, 18));
}

#[test]
fn quad_maths() {
    // Unscaled, centred: the corners are the clip-space corners.
    let m = Motion { position: [960.0, 540.0], scale: 100.0, rotation: 0.0, anchor: [960.0, 540.0], crop: [0.0; 4] };
    let q = quad(&m, (1920.0, 1080.0), (1920.0, 1080.0));
    assert_eq!(q[0], [-1.0, 1.0, 0.0, 0.0]);
    assert_eq!(q[3], [1.0, -1.0, 1.0, 1.0]);
    // 90° rotation turns the top-left corner to the top-right.
    let r = quad(&Motion { rotation: 90.0, anchor: [540.0, 540.0], ..m }, (1080.0, 1080.0), (1920.0, 1080.0));
    assert!((r[0][1] - 1.0).abs() < 1e-4);
}
