//! The standard pack, loaded the way the app loads it: through the public
//! ABI and the host's loader, which validates every shader.

use std::sync::Arc;
use ve_plugin_host::{native, EffectInfo, EffectKind};

fn pack() -> Vec<EffectInfo> {
    let lib = ve_builtins::linked().into_iter().map(|f| f()).find(|l| l.name() == "standard").expect("linked");
    native::load(Arc::from(lib), Some("standard")).unwrap_or_else(|e| panic!("{e}"))
}

#[test]
fn forty_effects_and_twenty_five_transitions_load() {
    let fx = pack();
    let count = |k| fx.iter().filter(|e| e.kind == k).count();
    assert_eq!(count(EffectKind::Filter) + count(EffectKind::Generator), 40);
    assert_eq!(count(EffectKind::Transition), 25);
    for e in &fx {
        assert!(e.plugin.id.starts_with(ve_builtins::standard::ID_PREFIX), "{}", e.plugin.id);
        assert!(!e.category.is_empty() && !e.name.is_empty(), "{}", e.plugin.id);
        for p in &e.params {
            assert!(p.min <= p.default[0] && p.default[0] <= p.max || matches!(p.kind, ve_plugin_host::ParamKind::Vec2 | ve_plugin_host::ParamKind::Color), "{}.{}", e.plugin.id, p.id);
        }
    }
    // Names are unique within a kind (the Effects panel lists them by name).
    let mut names: Vec<_> = fx.iter().map(|e| (e.kind == EffectKind::Transition, e.name.clone())).collect();
    names.sort();
    let n = names.len();
    names.dedup();
    assert_eq!(names.len(), n, "duplicate names");
}

// ---- on a GPU (skipped where there is none) ------------------------------

use ve_plugin_host::{intrinsic, ParamKind};
use ve_ports::{ColorTags, FrameData, PixelFormat, VideoFrame};
use ve_render::*;
use ve_time::Time;

const W: u32 = 64;
const H: u32 = 36;

fn gpu() -> Option<(wgpu::Device, wgpu::Queue)> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default())).ok()?;
    pollster::block_on(adapter.request_device(&Default::default())).ok()
}

/// An opaque RGBA picture, `f(x, y)` per pixel.
fn picture(f: impl Fn(u32, u32) -> [u8; 3]) -> Arc<VideoFrame> {
    let mut px = Vec::with_capacity((W * H * 4) as usize);
    for y in 0..H {
        for x in 0..W {
            let [r, g, b] = f(x, y);
            px.extend([r, g, b, 255]);
        }
    }
    Arc::new(VideoFrame {
        pts: Time::ZERO,
        duration: Time::from_seconds(1),
        width: W,
        height: H,
        format: PixelFormat::Rgba8,
        color: ColorTags { primaries: "bt709".into(), transfer: "srgb".into(), matrix: "bt709".into(), full_range: true },
        data: FrameData::Cpu { planes: vec![px], strides: vec![(W * 4) as usize] },
    })
}

/// Colour ramps and blocks: every pixel different.
fn busy() -> Arc<VideoFrame> {
    picture(|x, y| [(x * 4) as u8, (y * 7) as u8, if (x / 8 + y / 8) % 2 == 0 { 200 } else { 40 }])
}

/// Something else entirely, for the other side of a transition.
fn other() -> Arc<VideoFrame> {
    picture(|x, y| [255 - (y * 7) as u8, 90, (x * 3) as u8])
}

fn flat() -> Arc<VideoFrame> {
    picture(|_, _| [150, 110, 70])
}

fn layer(f: Arc<VideoFrame>) -> RenderLayer {
    let m = Motion { position: [W as f32 / 2.0, H as f32 / 2.0], scale: 100.0, rotation: 0.0, anchor: [W as f32 / 2.0, H as f32 / 2.0], crop: [0.0; 4] };
    RenderLayer::of_frame(f, m, 1.0)
}

/// An effect at its defaults, its colours converted as the engine does.
fn at_defaults(e: &EffectInfo, space: WorkingSpace) -> GpuEffect {
    let params = e
        .params
        .iter()
        .map(|p| {
            let v = p.default.map(|d| d as f32);
            if p.kind == ParamKind::Color { display_color_to_working(v, space) } else { v }
        })
        .collect();
    GpuEffect { key: format!("{}@{}", e.plugin.id, e.plugin.major_version), wgsl: e.wgsl.as_deref().unwrap().into(), params, time: 0.5 }
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    c: Compositor,
}

impl Gpu {
    fn new() -> Option<Self> {
        let (device, queue) = gpu()?;
        let c = Compositor::new(&device);
        Some(Gpu { device, queue, c })
    }

    fn render(&mut self, layers: &[RenderLayer], what: &str) -> Vec<u8> {
        self.render_in(layers, what, WorkingSpace::AcesCg)
    }

    fn render_in(&mut self, layers: &[RenderLayer], what: &str, space: WorkingSpace) -> Vec<u8> {
        let plan = FramePlan { time: Time::ZERO, width: W, height: H, layers: vec![], proxies: false };
        self.c.render(&self.device, &self.queue, &plan, layers, (W, H), space);
        assert!(self.c.errors.is_empty(), "{what}: {:?}", self.c.errors);
        self.c.read_output(&self.device, &self.queue).unwrap().2
    }
}

fn with(f: Arc<VideoFrame>, e: GpuEffect) -> RenderLayer {
    RenderLayer { effects: vec![e], ..layer(f) }
}

fn across(a: Arc<VideoFrame>, b: Arc<VideoFrame>, e: GpuEffect, progress: f32) -> RenderLayer {
    RenderLayer { transition: Some(Box::new(RenderTransition { effect: e, progress, incoming: Some(layer(b)), self_incoming: false })), ..layer(a) }
}

/// The largest difference between two pictures, over the pixels `keep` picks.
fn worst(a: &[u8], b: &[u8], keep: impl Fn(u32, u32) -> bool) -> (u8, (u32, u32)) {
    let mut out = (0, (0, 0));
    for y in 0..H {
        for x in (0..W).filter(|&x| keep(x, y)) {
            let i = ((y * W + x) * 4) as usize;
            for k in 0..4 {
                let d = a[i + k].abs_diff(b[i + k]);
                if d > out.0 {
                    out = (d, (x, y));
                }
            }
        }
    }
    out
}

#[test]
fn every_shader_compiles_and_renders_on_the_gpu() {
    let Some(mut g) = Gpu::new() else { return eprintln!("skipped: no GPU") };
    for e in pack() {
        let fx = at_defaults(&e, WorkingSpace::AcesCg);
        let l = match e.kind {
            EffectKind::Filter => with(busy(), fx),
            EffectKind::Transition => across(busy(), other(), fx, 0.4),
            EffectKind::Generator => RenderLayer { source: LayerSource::Shader(fx), ..layer(busy()) },
        };
        let img = g.render(&[l], &e.plugin.id);
        // Something was drawn (no effect at its defaults blanks the frame).
        assert!(img.chunks(4).any(|p| p[3] > 0), "{}: nothing drawn", e.plugin.id);
    }
}

/// Every video transition is exactly the outgoing picture at 0 and the
/// incoming one at 1: no jump at either end. (In linear Rec.709: in ACEScg
/// one half-float rounding in a shader's arithmetic shows near black, and
/// GPUs differ in where they round — lavapipe does, Metal does not.)
#[test]
fn transitions_start_on_a_and_end_on_b() {
    let Some(mut g) = Gpu::new() else { return eprintln!("skipped: no GPU") };
    let space = WorkingSpace::LinearRec709;
    let a = g.render_in(&[layer(busy())], "A", space);
    let b = g.render_in(&[layer(other())], "B", space);
    let builtin = intrinsic::all().into_iter().filter(|e| e.kind == EffectKind::Transition && e.wgsl.is_some());
    for e in pack().into_iter().filter(|e| e.kind == EffectKind::Transition).chain(builtin) {
        let fx = at_defaults(&e, space);
        for (p, want, side) in [(0.0, &a, "A"), (1.0, &b, "B")] {
            let got = g.render_in(&[across(busy(), other(), fx.clone(), p)], &e.plugin.id, space);
            let (d, at) = worst(&got, want, |_, _| true);
            assert!(d <= 1, "{} at {p}: not {side} (off by {d} at {at:?})", e.plugin.id);
        }
    }
}

/// Colour corrections at their defaults change nothing. (Checked in
/// linear Rec.709: in ACEScg a Rec.709 colour with a zero channel reaches
/// the monitor through large terms that cancel, so one half-float rounding
/// in any extra pass shows near black as a few levels.)
#[test]
fn colour_corrections_are_neutral_at_their_defaults() {
    let Some(mut g) = Gpu::new() else { return eprintln!("skipped: no GPU") };
    let space = WorkingSpace::LinearRec709;
    let plain = g.render_in(&[layer(busy())], "plain", space);
    let fx = pack();
    for id in ["brightness_contrast", "levels", "hue_saturation", "channel_mixer", "color_balance"] {
        let e = fx.iter().find(|e| e.plugin.id == format!("{}{id}", ve_builtins::standard::ID_PREFIX)).unwrap();
        let got = g.render_in(&[with(busy(), at_defaults(e, space))], id, space);
        let (d, at) = worst(&got, &plain, |_, _| true);
        let i = ((at.1 * W + at.0) * 4) as usize;
        assert!(d <= 1, "{id}: off by {d} at {at:?}: {:?} vs {:?}", &got[i..i + 4], &plain[i..i + 4]);
    }
}

/// Blurs and sharpens leave a flat picture flat (away from its edges,
/// where a blur may let transparency in).
#[test]
fn blurs_and_sharpens_keep_a_flat_picture_flat() {
    let Some(mut g) = Gpu::new() else { return eprintln!("skipped: no GPU") };
    let plain = g.render(&[layer(flat())], "plain");
    let fx = pack();
    for e in fx.iter().filter(|e| e.category == "Blur & Sharpen") {
        let got = g.render(&[with(flat(), at_defaults(e, WorkingSpace::AcesCg))], &e.plugin.id);
        let (d, at) = worst(&got, &plain, |x, y| (12..W - 12).contains(&x) && (12..H - 12).contains(&y));
        assert!(d <= 1, "{}: off by {d} at {at:?}", e.plugin.id);
    }
}


/// Every effect at its defaults, and every transition at 35 %, on a test
/// picture: written to `target/fx-look/sheet.ppm` to be looked at.
#[test]
#[ignore = "writes a contact sheet; run by hand"]
fn contact_sheet() {
    let Some((device, queue)) = gpu() else { return };
    let (w, h) = (192u32, 108u32);
    let pic = |f: &dyn Fn(f32, f32) -> [f32; 3]| {
        let mut px = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let c = f(x as f32 / w as f32, y as f32 / h as f32);
                px.extend(c.map(|v| (v.clamp(0.0, 1.0) * 255.0) as u8));
                px.push(255);
            }
        }
        Arc::new(VideoFrame {
            pts: Time::ZERO,
            duration: Time::from_seconds(1),
            width: w,
            height: h,
            format: PixelFormat::Rgba8,
            color: ColorTags { primaries: "bt709".into(), transfer: "srgb".into(), matrix: "bt709".into(), full_range: true },
            data: FrameData::Cpu { planes: vec![px], strides: vec![(w * 4) as usize] },
        })
    };
    // A sky, a green hill, a red sun and a white bar of text-like blocks.
    let a = pic(&|x, y| {
        let sun = ((x - 0.7).powi(2) * 3.2 + (y - 0.3).powi(2)).sqrt() < 0.12;
        let hill = y > 0.62 + 0.08 * (x * 9.0).sin();
        let bar = (0.1..0.45).contains(&x) && (0.2..0.3).contains(&y) && ((x * 40.0) as i32 % 3 != 0);
        if bar { [0.95, 0.95, 0.95] } else if sun { [0.95, 0.25, 0.1] } else if hill { [0.15, 0.55 - y * 0.3, 0.12] } else { [0.35 + y * 0.3, 0.55 + y * 0.3, 0.9] }
    });
    let b = pic(&|x, y| [0.9 - y * 0.5, 0.75 * x, 0.2 + 0.6 * ((x * 6.0).floor() as i32 % 2) as f32]);
    let mut c = Compositor::new(&device);
    let mut tiles = Vec::new();
    for e in pack() {
        let fx = at_defaults(&e, WorkingSpace::AcesCg);
        let m = Motion { position: [w as f32 / 2.0, h as f32 / 2.0], scale: 100.0, rotation: 0.0, anchor: [w as f32 / 2.0, h as f32 / 2.0], crop: [0.0; 4] };
        let base = RenderLayer::of_frame(a.clone(), m, 1.0);
        let l = match e.kind {
            EffectKind::Filter => RenderLayer { effects: vec![fx], ..base },
            EffectKind::Generator => RenderLayer { source: LayerSource::Shader(fx), ..base },
            EffectKind::Transition => RenderLayer {
                transition: Some(Box::new(RenderTransition { effect: fx, progress: 0.35, incoming: Some(RenderLayer::of_frame(b.clone(), m, 1.0)), self_incoming: false })),
                ..base
            },
        };
        let plan = FramePlan { time: Time::ZERO, width: w, height: h, layers: vec![], proxies: false };
        c.render(&device, &queue, &plan, &[l], (w, h), WorkingSpace::AcesCg);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        tiles.push(c.read_output(&device, &queue).unwrap().2);
    }
    let cols = 8u32;
    let rows = (tiles.len() as u32).div_ceil(cols);
    let (sw, sh) = (cols * (w + 4), rows * (h + 4));
    let mut sheet = vec![40u8; (sw * sh * 3) as usize];
    for (i, t) in tiles.iter().enumerate() {
        let (ox, oy) = ((i as u32 % cols) * (w + 4), (i as u32 / cols) * (h + 4));
        for y in 0..h {
            for x in 0..w {
                let s = ((y * w + x) * 4) as usize;
                let d = (((oy + y) * sw + ox + x) * 3) as usize;
                sheet[d..d + 3].copy_from_slice(&t[s..s + 3]);
            }
        }
    }
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fx-look");
    std::fs::create_dir_all(&dir).unwrap();
    let mut out = format!("P6\n{sw} {sh}\n255\n").into_bytes();
    out.extend(sheet);
    std::fs::write(dir.join("sheet.ppm"), out).unwrap();
    for (i, e) in pack().iter().enumerate() {
        println!("{i:2} {}", e.name);
    }
}
