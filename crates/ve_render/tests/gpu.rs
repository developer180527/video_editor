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
    // Luma 126 (limited) is 0.502 encoded; it must come back as 128 ± 1 —
    // and grey: chroma 128 is exactly neutral (a 0.5 midpoint tinted it).
    let p = px(&img, 32, 18);
    assert!((127..=129).contains(&p[1]), "{p:?}");
    assert!(p[0] == p[1] && p[1] == p[2], "neutral grey picked up a tint: {p:?}");
}

/// The texture values of 10-bit limited-range levels: P010 keeps them in
/// the top bits of 16, so they read as v·64/65535.
#[test]
fn levels_are_exact() {
    let read10 = |v: u32| (v * 64) as f32 / 65535.0;
    let (yo, ys, cs, cm) = yuv_levels(true, false);
    assert!(((read10(64) - yo) * ys).abs() < 1e-6, "10-bit black");
    assert!(((read10(940) - yo) * ys - 1.0).abs() < 1e-6, "10-bit white");
    assert!((read10(512) - cm).abs() < 1e-7, "10-bit neutral chroma");
    assert!(((read10(960) - cm) * cs - 0.5).abs() < 1e-6, "10-bit chroma maximum");
    let (yo, ys, _, cm) = yuv_levels(false, false);
    assert!(((235.0 / 255.0 - yo) * ys - 1.0).abs() < 1e-6, "8-bit white");
    assert_eq!(cm, 128.0 / 255.0, "8-bit neutral chroma");
}

/// 10-bit white and grey through the GPU (where 16-bit textures exist).
#[test]
fn p010_white_is_white_and_grey_is_grey() {
    let Some((device, queue)) = gpu() else { return eprintln!("skipped: no GPU") };
    if !device.features().contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM) {
        return eprintln!("skipped: no 16-bit textures");
    }
    let p010 = |y10: u16| {
        let (w, h) = (64u32, 36u32);
        let word = |v: u16| (v << 6).to_le_bytes();
        let luma: Vec<u8> = (0..w * h).flat_map(|_| word(y10)).collect();
        let chroma: Vec<u8> = (0..w * h / 2).flat_map(|_| word(512)).collect();
        Arc::new(VideoFrame {
            pts: Time::ZERO,
            duration: Time::from_seconds(1),
            width: w,
            height: h,
            format: PixelFormat::P010,
            color: ColorTags { primaries: "bt709".into(), transfer: "bt709".into(), matrix: "bt709".into(), full_range: false },
            data: FrameData::Cpu { planes: vec![luma, chroma], strides: vec![w as usize * 2, w as usize * 2] },
        })
    };
    let mut c = Compositor::new(&device);
    let mut shot = |f| {
        c.render(&device, &queue, &plan(64, 36), &[layer(f, 100.0, 1.0)], (64, 36), WorkingSpace::AcesCg);
        px(&c.read_output(&device, &queue).unwrap(), 32, 18)
    };
    assert_eq!(&shot(p010(940))[..3], &[255, 255, 255], "10-bit white");
    let grey = shot(p010(505)); // (505-64)/876 = 0.503: clear of a rounding edge
    assert!(grey[0] == grey[1] && grey[1] == grey[2] && (127..=129).contains(&grey[1]), "10-bit grey: {grey:?}");
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

/// A full-range NV12 grey at `code`/255 with the given transfer, in BT.2020.
fn hdr_gray(code: u8, transfer: &str) -> Arc<VideoFrame> {
    let (w, h) = (64u32, 36u32);
    Arc::new(VideoFrame {
        pts: Time::ZERO,
        duration: Time::from_seconds(1),
        width: w,
        height: h,
        format: PixelFormat::Nv12,
        color: ColorTags { primaries: "bt2020".into(), transfer: transfer.into(), matrix: "bt2020nc".into(), full_range: true },
        data: FrameData::Cpu { planes: vec![vec![code; (w * h) as usize], vec![128; (w * h / 2) as usize]], strides: vec![w as usize, w as usize] },
    })
}

#[test]
fn hdr_reference_white_is_display_white() {
    let shade = |f| render(&[layer(f, 100.0, 1.0)], WorkingSpace::AcesCg).map(|img| px(&img, 32, 18)[1]);
    // HLG 75% is reference white: near the top, not two stops down (~146).
    let Some(hlg) = shade(hdr_gray(191, "arib-std-b67")) else { return eprintln!("skipped: no GPU") };
    assert!((238..=252).contains(&hlg), "HLG reference white shows as {hlg}");
    // PQ 100 cd/m² is half of reference white: mid-grey, not clipped (255).
    let pq = shade(hdr_gray(130, "smpte2084")).unwrap();
    assert!((185..=197).contains(&pq), "PQ 100 cd/m² shows as {pq}");
    // PQ 1000 cd/m² highlights roll off into range rather than clip hard.
    let bright = shade(hdr_gray(192, "smpte2084")).unwrap();
    assert!(bright >= 250, "PQ 1000 cd/m² shows as {bright}");
}

#[test]
fn a_readback_is_not_disturbed_by_the_next_render() {
    let Some((device, queue)) = gpu() else { return eprintln!("skipped: no GPU") };
    let mut c = Compositor::new(&device);
    c.render(&device, &queue, &plan(64, 36), &[layer(gray(64, 36, 16), 100.0, 1.0)], (64, 36), WorkingSpace::AcesCg);
    let black = c.start_readback(&device, &queue).unwrap();
    c.render(&device, &queue, &plan(64, 36), &[layer(gray(64, 36, 235), 100.0, 1.0)], (64, 36), WorkingSpace::AcesCg);
    let white = c.start_readback(&device, &queue).unwrap();
    assert!(px(&black.finish(&device).unwrap(), 32, 18)[0] < 5);
    assert!(px(&white.finish(&device).unwrap(), 32, 18)[0] > 250);
}

/// A 1-pixel black/white checkerboard shrunk to a fifth: with a mip chain it
/// averages to an even grey. Bilinear alone samples texel centres at 5× and
/// aliases into a coarse black/white checker.
#[test]
fn downscaled_layers_are_filtered() {
    let (w, h) = (320u32, 180u32);
    let luma: Vec<u8> = (0..h).flat_map(|y| (0..w).map(move |x| if (x + y) % 2 == 0 { 16 } else { 235 })).collect();
    let frame = Arc::new(VideoFrame {
        pts: Time::ZERO,
        duration: Time::from_seconds(1),
        width: w,
        height: h,
        format: PixelFormat::Nv12,
        color: ColorTags { primaries: "bt709".into(), transfer: "bt709".into(), matrix: "bt709".into(), full_range: false },
        data: FrameData::Cpu { planes: vec![luma, vec![128; (w * h / 2) as usize]], strides: vec![w as usize, w as usize] },
    });
    let Some(img) = render(&[layer(frame, 20.0, 1.0)], WorkingSpace::LinearRec709) else { return eprintln!("skipped: no GPU") };
    // Linear average of black and white is 0.5 → 0.5^(1/2.4) ≈ 191, everywhere.
    for (x, y) in [(10, 10), (31, 17), (32, 18), (50, 30)] {
        let c = px(&img, x, y)[0];
        assert!((184..=198).contains(&c), "({x},{y}) = {c}");
    }
}

/// Rendering again with nothing changed gives the same picture (cached
/// buffers and bind groups are rewritten, not stale), and a new frame in
/// the same slot shows.
#[test]
fn steady_and_changing_frames_render_right() {
    let Some((device, queue)) = gpu() else { return eprintln!("skipped: no GPU") };
    let mut c = Compositor::new(&device);
    let mut shot = |l: &RenderLayer| {
        c.render(&device, &queue, &plan(64, 36), std::slice::from_ref(l), (64, 36), WorkingSpace::AcesCg);
        px(&c.read_output(&device, &queue).unwrap(), 32, 18)[0]
    };
    let dark = layer(gray(64, 36, 60), 100.0, 1.0);
    let a = shot(&dark);
    assert_eq!(shot(&dark), a);
    let light = layer(gray(64, 36, 200), 100.0, 1.0);
    assert!(shot(&light) > a + 50);
    let half = RenderLayer { opacity: 0.5, ..light.clone() };
    assert!(shot(&half) < shot(&light));
}
