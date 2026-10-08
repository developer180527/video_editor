//! The wgpu compositor.
//!
//! Per frame, for each layer bottom to top:
//! 1. **Decode pass**: the frame's YUV planes → premultiplied, scene-linear
//!    RGBA in the working space (ACEScg by default), at source size. Matrix,
//!    range, transfer function and primaries come from the frame's tags.
//! 2. **Effect passes**: each GPU effect (a plugin's WGSL) ping-pongs over
//!    the layer's own texture.
//! 3. **Composite pass**: the layer, cropped and transformed by Motion, is
//!    blended with its opacity into the working target (RGBA16F).
//!
//! Then a **display pass** turns the working image into the monitor signal
//! (Rec.709 primaries, BT.1886 2.4 gamma) in an RGBA8 texture the UI shows.
//! A Rec.709 source therefore round-trips exactly.
//!
//! Colour here is built-in maths for the common spaces. OpenColorIO replaces
//! the transfer/primaries tables when it is wired in; the passes stay.
//!
//! Scale: 1.0 is SDR reference white. HDR sources (PQ, HLG) are placed so
//! their reference white (203 cd/m², BT.2408) lands there too, and — while
//! the only output is SDR — their highlights are rolled off into range in
//! the decode pass, so an HDR clip looks right next to an SDR one instead of
//! dark (HLG) or blown out (PQ).

use std::collections::HashMap;
use std::sync::Arc;

use ve_ports::{FrameData, PixelFormat, VideoFrame};

use crate::FramePlan;

/// The engine's working format: half-float RGBA, linear light.
pub const WORKING_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// What the monitor shows: display-encoded RGBA8.
pub const DISPLAY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Turns a decoded frame into a GPU texture without the CPU path: a
/// `CVPixelBuffer` → Metal texture on Apple. Platforms supply one; until
/// then frames come as CPU planes.
pub trait TextureImporter: Send + Sync {
    fn import(&self, device: &wgpu::Device, queue: &wgpu::Queue, frame: &VideoFrame) -> Option<wgpu::Texture>;
}

/// Motion, in sequence pixels and degrees; crop in percent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub position: [f32; 2],
    /// Percent.
    pub scale: f32,
    pub rotation: f32,
    pub anchor: [f32; 2],
    /// Left, top, right, bottom, percent.
    pub crop: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Blend {
    Normal,
    Multiply,
    Screen,
    Add,
    Overlay,
}

/// A plugin effect ready for the GPU: its shader and its parameter values in
/// declaration order.
#[derive(Clone, Debug)]
pub struct GpuEffect {
    /// Identifies the compiled pipeline (the plugin id and version).
    pub key: String,
    pub wgsl: Arc<str>,
    pub params: Vec<[f32; 4]>,
    pub time: f32,
}

/// One layer, with everything resolved.
#[derive(Clone)]
pub struct RenderLayer {
    pub frame: Arc<VideoFrame>,
    pub motion: Motion,
    /// 0..1.
    pub opacity: f32,
    pub blend: Blend,
    pub effects: Vec<GpuEffect>,
}

/// Which colour space the sequence works in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkingSpace {
    AcesCg,
    LinearRec709,
}

impl WorkingSpace {
    pub fn from_name(name: &str) -> Self {
        let n = name.to_ascii_lowercase();
        if n.contains("acescg") || n.contains("ap1") {
            WorkingSpace::AcesCg
        } else {
            WorkingSpace::LinearRec709
        }
    }
}

const COMMON: &str = r#"
struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// A full-screen triangle.
@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> VsOut {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VsOut;
    o.position = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}
"#;

const DECODE: &str = r#"
struct Decode {
    // Columns of the YCbCr → RGB matrix, then (y offset, y scale, c scale, transfer id).
    m0: vec4<f32>,
    m1: vec4<f32>,
    m2: vec4<f32>,
    range: vec4<f32>,
    // Columns of the source-primaries → working-space matrix.
    p0: vec4<f32>,
    p1: vec4<f32>,
    p2: vec4<f32>,
};
@group(0) @binding(0) var<uniform> d: Decode;
@group(0) @binding(1) var luma: texture_2d<f32>;
@group(0) @binding(2) var chroma: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

fn bt1886(v: f32) -> f32 { return pow(max(v, 0.0), 2.4); }
fn srgb(v: f32) -> f32 {
    if (v <= 0.04045) { return v / 12.92; }
    return pow((v + 0.055) / 1.055, 2.4);
}
// HDR reference white, cd/m² (BT.2408): maps to 1.0.
const REF_WHITE: f32 = 203.0;
fn pq(v: f32) -> f32 {
    let m1 = 0.1593017578125; let m2 = 78.84375;
    let c1 = 0.8359375; let c2 = 18.8515625; let c3 = 18.6875;
    let p = pow(max(v, 0.0), 1.0 / m2);
    // Absolute: 1.0 is 10 000 cd/m².
    return pow(max(p - c1, 0.0) / (c2 - c3 * p), 1.0 / m1) * (10000.0 / REF_WHITE);
}
fn hlg_scene(v: f32) -> f32 {
    let a = 0.17883277; let b = 0.28466892; let c = 0.55991073;
    if (v <= 0.5) { return v * v / 3.0; }
    return (exp((v - c) / a) + b) / 12.0;
}
// HLG: inverse OETF to scene light, then the BT.2100 OOTF for a 1000 cd/m²
// display (system gamma 1.2, applied to BT.2020 luminance), in cd/m².
fn hlg(v: vec3<f32>) -> vec3<f32> {
    let e = vec3<f32>(hlg_scene(v.r), hlg_scene(v.g), hlg_scene(v.b));
    let ys = dot(e, vec3<f32>(0.2627, 0.6780, 0.0593));
    return e * pow(max(ys, 1e-6), 0.2) * (1000.0 / REF_WHITE);
}
fn to_linear(v: vec3<f32>, id: f32) -> vec3<f32> {
    if (id < 0.5) { return vec3<f32>(bt1886(v.r), bt1886(v.g), bt1886(v.b)); }
    if (id < 1.5) { return vec3<f32>(srgb(v.r), srgb(v.g), srgb(v.b)); }
    if (id < 2.5) { return vec3<f32>(pq(v.r), pq(v.g), pq(v.b)); }
    if (id < 3.5) { return hlg(v); }
    return v; // already linear
}
// Highlight roll-off for HDR sources on an SDR output: identity up to
// KNEE, then an exponential shoulder approaching 1.0 (slope-continuous).
fn shoulder(x: f32) -> f32 {
    let knee = 0.8;
    if (x <= knee) { return x; }
    return knee + (1.0 - knee) * (1.0 - exp(-(x - knee) / (1.0 - knee)));
}

@fragment
fn fs_decode(i: VsOut) -> @location(0) vec4<f32> {
    let y = (textureSample(luma, samp, i.uv).r - d.range.x) * d.range.y;
    let c = (textureSample(chroma, samp, i.uv).rg - vec2<f32>(0.5, 0.5)) * d.range.z;
    let m = mat3x3<f32>(d.m0.xyz, d.m1.xyz, d.m2.xyz);
    let encoded = clamp(m * vec3<f32>(y, c.x, c.y), vec3<f32>(0.0), vec3<f32>(1.0));
    let p = mat3x3<f32>(d.p0.xyz, d.p1.xyz, d.p2.xyz);
    var lin = p * to_linear(encoded, d.range.w);
    if (d.range.w > 1.5 && d.range.w < 3.5) {
        lin = vec3<f32>(shoulder(lin.r), shoulder(lin.g), shoulder(lin.b));
    }
    return vec4<f32>(lin, 1.0);
}
"#;

const COMPOSITE: &str = r#"
struct Quad {
    // Corners in clip space (xy) with their texture coordinates (zw):
    // top-left, top-right, bottom-left, bottom-right.
    c: array<vec4<f32>, 4>,
    opacity: vec4<f32>,
};
@group(0) @binding(0) var<uniform> q: Quad;
@group(0) @binding(1) var layer: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct QOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_quad(@builtin(vertex_index) i: u32) -> QOut {
    let idx = array<u32, 6>(0u, 1u, 2u, 2u, 1u, 3u);
    let c = q.c[idx[i]];
    var o: QOut;
    o.position = vec4<f32>(c.xy, 0.0, 1.0);
    o.uv = c.zw;
    return o;
}

@fragment
fn fs_quad(i: QOut) -> @location(0) vec4<f32> {
    return textureSample(layer, samp, i.uv) * q.opacity.x;
}
"#;

const DISPLAY: &str = r#"
struct Display {
    // Columns of the working → Rec.709 matrix.
    w0: vec4<f32>,
    w1: vec4<f32>,
    w2: vec4<f32>,
};
@group(0) @binding(0) var<uniform> d: Display;
@group(0) @binding(1) var work: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

@fragment
fn fs_display(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(work, samp, i.uv);
    // Over black, then to the display: Rec.709 primaries, BT.1886 (2.4).
    let m = mat3x3<f32>(d.w0.xyz, d.w1.xyz, d.w2.xyz);
    let rgb = clamp(m * c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(pow(rgb, vec3<f32>(1.0 / 2.4)), 1.0);
}
"#;

/// Declarations every plugin shader sees (the contract in sdk/WGSL_CONTRACT.md).
const EFFECT_HEADER: &str = r#"
struct EffectIn {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
struct Params {
    values: array<vec4<f32>, 64>,
    time: f32,
    progress: f32,
    scale: f32,
    _pad: f32,
};
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;
@group(0) @binding(3) var source_b: texture_2d<f32>;

@vertex
fn ve_effect_vs(@builtin(vertex_index) i: u32) -> EffectIn {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: EffectIn;
    o.position = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}
"#;

type M3 = [[f32; 3]; 3];
// Published matrices, kept as published.
#[allow(clippy::excessive_precision)]
mod matrices {
    use super::M3;
pub(super) const IDENTITY: M3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
pub(super) const REC709_TO_AP1: M3 = [[0.6131324, 0.3395380, 0.0474167], [0.0701244, 0.9163940, 0.0134515], [0.0205877, 0.1095746, 0.8697854]];
pub(super) const REC2020_TO_AP1: M3 = [[0.9748950, 0.0195991, 0.0055059], [0.0021796, 0.9955355, 0.0022850], [0.0047972, 0.0245320, 0.9706707]];
pub(super) const REC2020_TO_709: M3 = [[1.6604910, -0.5876411, -0.0728499], [-0.1245505, 1.1328999, -0.0083494], [-0.0181508, -0.1005789, 1.1187297]];
pub(super) const AP1_TO_709: M3 = [[1.7048587, -0.6217160, -0.0832994], [-0.1300768, 1.1407358, -0.0105598], [-0.0239641, -0.1289755, 1.1530140]];
}
use matrices::*;

/// Row-major 3×3 → three column vectors for WGSL `mat3x3` constructors.
fn columns(m: M3) -> [[f32; 4]; 3] {
    [0, 1, 2].map(|c| [m[0][c], m[1][c], m[2][c], 0.0])
}

/// YCbCr → R'G'B' matrix (row-major) for a matrix tag.
fn ycbcr(matrix: &str, height: u32) -> M3 {
    let (kr, kb) = match matrix {
        "bt470bg" | "smpte170m" | "bt601" => (0.299, 0.114),
        "bt2020nc" | "bt2020c" => (0.2627, 0.0593),
        "bt709" => (0.2126, 0.0722),
        // Untagged: HD and up is 709, SD is 601.
        _ if height >= 720 => (0.2126, 0.0722),
        _ => (0.299, 0.114),
    };
    let kg = 1.0 - kr - kb;
    [[1.0, 0.0, 2.0 - 2.0 * kr], [1.0, -(2.0 - 2.0 * kb) * kb / kg, -(2.0 - 2.0 * kr) * kr / kg], [1.0, 2.0 - 2.0 * kb, 0.0]]
}

fn transfer_id(transfer: &str) -> f32 {
    match transfer {
        "iec61966-2-1" => 1.0,
        "smpte2084" => 2.0,
        "arib-std-b67" => 3.0,
        "linear" => 4.0,
        _ => 0.0, // bt709, smpte170m, bt2020-10/12, unknown: BT.1886
    }
}

fn primaries_to(work: WorkingSpace, primaries: &str) -> M3 {
    let wide = primaries == "bt2020";
    match (work, wide) {
        (WorkingSpace::AcesCg, false) => REC709_TO_AP1,
        (WorkingSpace::AcesCg, true) => REC2020_TO_AP1,
        (WorkingSpace::LinearRec709, true) => REC2020_TO_709,
        (WorkingSpace::LinearRec709, false) => IDENTITY,
    }
}

fn texture(device: &wgpu::Device, label: &str, w: u32, h: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage,
        view_formats: &[],
    })
}

fn uniform(device: &wgpu::Device, bytes: &[u8]) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("uniform"), contents: bytes, usage: wgpu::BufferUsages::UNIFORM })
}

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

struct Planes {
    luma: wgpu::Texture,
    chroma: wgpu::Texture,
    key: (u32, u32, bool),
    /// The frame these textures hold, so an unchanged frame is not re-uploaded.
    holds: usize,
}

/// Textures that keep their size between frames: target, display, planes.
struct Sized {
    tex: wgpu::Texture,
    w: u32,
    h: u32,
}

pub struct Compositor {
    sixteen_bit: bool,
    sampler: wgpu::Sampler,
    decode: wgpu::RenderPipeline,
    composite: HashMap<Blend, wgpu::RenderPipeline>,
    display: wgpu::RenderPipeline,
    effects: HashMap<String, Result<wgpu::RenderPipeline, String>>,
    effect_layout: wgpu::BindGroupLayout,
    work: Option<Sized>,
    out: Option<Sized>,
    planes: Vec<Planes>,
    layer_tex: Vec<Sized>,
    dummy: wgpu::Texture,
    /// Effects that failed to compile, for the UI to report.
    pub errors: Vec<String>,
}

const LAYER_USAGE: wgpu::TextureUsages = wgpu::TextureUsages::RENDER_ATTACHMENT.union(wgpu::TextureUsages::TEXTURE_BINDING);

fn layout_entries(n_textures: u32) -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut v = vec![wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }];
    let tex = |binding| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
        count: None,
    };
    v.push(tex(1));
    if n_textures == 2 {
        v.push(tex(2));
        v.push(wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None });
    } else {
        v.push(wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None });
        if n_textures == 3 {
            v.push(tex(3)); // effects: source_b
        }
    }
    v
}

fn pipeline(device: &wgpu::Device, src: &str, vs: &str, fs: &str, layout: &wgpu::BindGroupLayout, format: wgpu::TextureFormat, blend: Option<wgpu::BlendState>) -> wgpu::RenderPipeline {
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(fs), source: wgpu::ShaderSource::Wgsl(src.into()) });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[Some(layout)], immediate_size: 0 });
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some(fs),
        layout: Some(&pl),
        vertex: wgpu::VertexState { module: &module, entry_point: Some(vs), buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState {
            module: &module,
            entry_point: Some(fs),
            targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
            compilation_options: Default::default(),
        }),
        primitive: Default::default(),
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
}

fn blend_state(b: Blend) -> wgpu::BlendState {
    use wgpu::{BlendComponent as C, BlendFactor as F, BlendOperation as O};
    let over = C { src_factor: F::One, dst_factor: F::OneMinusSrcAlpha, operation: O::Add };
    let color = match b {
        Blend::Add => C { src_factor: F::One, dst_factor: F::One, operation: O::Add },
        Blend::Screen => C { src_factor: F::One, dst_factor: F::OneMinusSrc, operation: O::Add },
        Blend::Multiply => C { src_factor: F::Dst, dst_factor: F::OneMinusSrcAlpha, operation: O::Add },
        // Overlay needs the destination in the shader; Normal until then.
        Blend::Normal | Blend::Overlay => over,
    };
    wgpu::BlendState { color, alpha: over }
}

impl Compositor {
    pub fn new(device: &wgpu::Device) -> Self {
        let sixteen_bit = device.features().contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mk_layout = |n| device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &layout_entries(n) });
        let decode_layout = mk_layout(2);
        let one_layout = mk_layout(1);
        let effect_layout = mk_layout(3);
        let decode = pipeline(device, &format!("{COMMON}{DECODE}"), "vs_full", "fs_decode", &decode_layout, WORKING_FORMAT, None);
        let composite = [Blend::Normal, Blend::Multiply, Blend::Screen, Blend::Add, Blend::Overlay]
            .into_iter()
            .map(|b| (b, pipeline(device, COMPOSITE, "vs_quad", "fs_quad", &one_layout, WORKING_FORMAT, Some(blend_state(b)))))
            .collect();
        let display = pipeline(device, &format!("{COMMON}{DISPLAY}"), "vs_full", "fs_display", &one_layout, DISPLAY_FORMAT, None);
        let dummy = texture(device, "dummy", 1, 1, WORKING_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING);
        Compositor {
            sixteen_bit,
            sampler,
            decode,
            composite,
            display,
            effects: HashMap::new(),
            effect_layout,
            work: None,
            out: None,
            planes: Vec::new(),
            layer_tex: Vec::new(),
            dummy,
            errors: Vec::new(),
        }
    }

    /// The monitor texture (display-encoded RGBA8) from the last render.
    pub fn output(&self) -> Option<&wgpu::Texture> {
        self.out.as_ref().map(|s| &s.tex)
    }

    fn sized(slot: &mut Option<Sized>, device: &wgpu::Device, label: &str, w: u32, h: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) {
        if !matches!(slot, Some(s) if s.w == w && s.h == h) {
            *slot = Some(Sized { tex: texture(device, label, w, h, format, usage), w, h });
        }
    }

    fn bind(&self, device: &wgpu::Device, layout: &wgpu::BindGroupLayout, buf: &wgpu::Buffer, views: &[&wgpu::TextureView]) -> wgpu::BindGroup {
        let mut entries = vec![wgpu::BindGroupEntry { binding: 0, resource: buf.as_entire_binding() }];
        entries.push(wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(views[0]) });
        if views.len() == 2 {
            entries.push(wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(views[1]) });
            entries.push(wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) });
        } else {
            entries.push(wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) });
            if views.len() == 3 {
                entries.push(wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(views[2]) });
            }
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout, entries: &entries })
    }

    fn effect_pipeline(&mut self, device: &wgpu::Device, e: &GpuEffect) -> Option<&wgpu::RenderPipeline> {
        if !self.effects.contains_key(&e.key) {
            let src = format!("{EFFECT_HEADER}\n{}", e.wgsl);
            // A plugin's shader may not compile; catch it rather than abort.
            let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let p = pipeline(device, &src, "ve_effect_vs", "effect", &self.effect_layout, WORKING_FORMAT, None);
            let err = pollster::block_on(scope.pop());
            let r = match err {
                Some(e2) => {
                    self.errors.push(format!("{}: {e2}", e.key));
                    Err(e2.to_string())
                }
                None => Ok(p),
            };
            self.effects.insert(e.key.clone(), r);
        }
        self.effects.get(&e.key).and_then(|r| r.as_ref().ok())
    }

    /// Upload one frame's planes into slot `i`.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, i: usize, frame: &Arc<VideoFrame>) -> bool {
        let f: &VideoFrame = frame;
        let FrameData::Cpu { planes, strides } = &f.data else { return false };
        let deep = f.format == PixelFormat::P010 && self.sixteen_bit;
        let key = (f.width, f.height, deep);
        if self.planes.get(i).is_none_or(|p| p.key != key) {
            let (lf, cf) = if deep {
                (wgpu::TextureFormat::R16Unorm, wgpu::TextureFormat::Rg16Unorm)
            } else {
                (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm)
            };
            let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
            let p = Planes {
                luma: texture(device, "luma", f.width, f.height, lf, usage),
                chroma: texture(device, "chroma", f.width.div_ceil(2), f.height.div_ceil(2), cf, usage),
                key,
                holds: 0,
            };
            if i < self.planes.len() {
                self.planes[i] = p;
            } else {
                self.planes.push(p);
            }
        }
        // The same frame as last time (video slower than the display): done.
        let id = Arc::as_ptr(frame) as usize;
        if self.planes[i].holds == id {
            return true;
        }
        self.planes[i].holds = id;
        let p = &self.planes[i];
        // 10-bit without 16-bit textures: keep the top 8 bits.
        let squash = |plane: &[u8]| -> Vec<u8> { plane.as_chunks::<2>().0.iter().map(|c| c[1]).collect() };
        let (luma, chroma, ls, cs) = if f.format == PixelFormat::P010 && !deep {
            (squash(&planes[0]), squash(&planes[1]), strides[0] / 2, strides[1] / 2)
        } else {
            (planes[0].clone(), planes[1].clone(), strides[0], strides[1])
        };
        let write = |tex: &wgpu::Texture, data: &[u8], stride: usize, w: u32, h: u32| {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                data,
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(stride as u32), rows_per_image: Some(h) },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
        };
        write(&p.luma, &luma, ls, f.width, f.height);
        write(&p.chroma, &chroma, cs, f.width.div_ceil(2), f.height.div_ceil(2));
        true
    }

    /// Draw `layers` (bottom first) for `plan` and produce the monitor image.
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, plan: &FramePlan, layers: &[RenderLayer], seq_size: (u32, u32), space: WorkingSpace) {
        let (w, h) = (plan.width, plan.height);
        let target_usage = LAYER_USAGE | wgpu::TextureUsages::COPY_SRC;
        Self::sized(&mut self.work, device, "work", w, h, WORKING_FORMAT, target_usage);
        Self::sized(&mut self.out, device, "monitor", w, h, DISPLAY_FORMAT, target_usage);
        let mut enc = device.create_command_encoder(&Default::default());
        let work_view = self.work.as_ref().unwrap().tex.create_view(&Default::default());
        {
            let _clear = begin(&mut enc, &work_view, Some(wgpu::Color::TRANSPARENT));
        }
        for (i, layer) in layers.iter().enumerate() {
            let f = &layer.frame;
            if !self.upload(device, queue, i, f) {
                continue;
            }
            // Layer texture pair for decode + effect ping-pong.
            while self.layer_tex.len() < 2 * (i + 1) {
                self.layer_tex.push(Sized { tex: texture(device, "layer", 1, 1, WORKING_FORMAT, LAYER_USAGE), w: 1, h: 1 });
            }
            for k in [2 * i, 2 * i + 1] {
                if self.layer_tex[k].w != f.width || self.layer_tex[k].h != f.height {
                    self.layer_tex[k] = Sized { tex: texture(device, "layer", f.width, f.height, WORKING_FORMAT, LAYER_USAGE), w: f.width, h: f.height };
                }
            }
            // 1. Decode.
            let m = columns(ycbcr(&f.color.matrix, f.height));
            let deep = f.format == PixelFormat::P010;
            let (yo, ys, cs) = if f.color.full_range { (0.0, 1.0, 1.0) } else if deep { (64.0 / 1023.0, 1023.0 / 876.0, 1023.0 / 896.0) } else { (16.0 / 255.0, 255.0 / 219.0, 255.0 / 224.0) };
            let pm = columns(primaries_to(space, &f.color.primaries));
            let mut u = Vec::new();
            for c in m {
                u.extend(c);
            }
            u.extend([yo, ys, cs, transfer_id(&f.color.transfer)]);
            for c in pm {
                u.extend(c);
            }
            let buf = uniform(device, &floats(&u));
            let (lv, cv) = (self.planes[i].luma.create_view(&Default::default()), self.planes[i].chroma.create_view(&Default::default()));
            let bg = self.bind(device, &self.decode.get_bind_group_layout(0), &buf, &[&lv, &cv]);
            let mut cur = 2 * i;
            {
                let view = self.layer_tex[cur].tex.create_view(&Default::default());
                let mut pass = begin(&mut enc, &view, Some(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(&self.decode);
                pass.set_bind_group(0, &bg, &[]);
                pass.draw(0..3, 0..1);
            }
            // 2. Effects.
            for e in &layer.effects {
                if self.effect_pipeline(device, e).is_none() {
                    continue;
                }
                let mut vals = vec![0f32; 64 * 4 + 4];
                for (k, p) in e.params.iter().take(64).enumerate() {
                    vals[k * 4..k * 4 + 4].copy_from_slice(p);
                }
                vals[256] = e.time;
                vals[258] = plan.width as f32 / seq_size.0.max(1) as f32;
                let buf = uniform(device, &floats(&vals));
                let src_view = self.layer_tex[cur].tex.create_view(&Default::default());
                let dummy = self.dummy.create_view(&Default::default());
                // Bindings: source, (unused slot), source_b.
                let bg = self.bind(device, &self.effect_layout, &buf, &[&src_view, &dummy, &dummy]);
                let next = cur ^ 1;
                let dst_view = self.layer_tex[next].tex.create_view(&Default::default());
                let pipe = self.effects[&e.key].as_ref().unwrap();
                let mut pass = begin(&mut enc, &dst_view, Some(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(pipe);
                pass.set_bind_group(0, &bg, &[]);
                pass.draw(0..3, 0..1);
                drop(pass);
                cur = next;
            }
            // 3. Composite.
            let corners = quad(&layer.motion, (f.width as f32, f.height as f32), (seq_size.0 as f32, seq_size.1 as f32));
            let mut u: Vec<f32> = corners.iter().flatten().copied().collect();
            u.extend([layer.opacity.clamp(0.0, 1.0), 0.0, 0.0, 0.0]);
            let buf = uniform(device, &floats(&u));
            let lview = self.layer_tex[cur].tex.create_view(&Default::default());
            let pipe = &self.composite[&layer.blend];
            let bg = self.bind(device, &pipe.get_bind_group_layout(0), &buf, &[&lview]);
            let mut pass = begin(&mut enc, &work_view, None);
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..6, 0..1);
        }
        // Display.
        let to_display = match space {
            WorkingSpace::AcesCg => AP1_TO_709,
            WorkingSpace::LinearRec709 => IDENTITY,
        };
        let u: Vec<f32> = columns(to_display).iter().flatten().copied().collect();
        let buf = uniform(device, &floats(&u));
        let bg = self.bind(device, &self.display.get_bind_group_layout(0), &buf, &[&work_view]);
        let out_view = self.out.as_ref().unwrap().tex.create_view(&Default::default());
        {
            let mut pass = begin(&mut enc, &out_view, Some(wgpu::Color::BLACK));
            pass.set_pipeline(&self.display);
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([enc.finish()]);
    }

    /// Copy the monitor image back as RGBA8 rows (export, tests).
    pub fn read_output(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<(u32, u32, Vec<u8>)> {
        let out = self.out.as_ref()?;
        let (w, h) = (out.w, out.h);
        let row = (w * 4).div_ceil(256) * 256;
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &out.tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        queue.submit([enc.finish()]);
        let slice = buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        rx.recv().ok()?.ok()?;
        let data = slice.get_mapped_range().ok()?;
        let mut px = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h as usize {
            px.extend_from_slice(&data[y * row as usize..y * row as usize + w as usize * 4]);
        }
        drop(data);
        buf.unmap();
        Some((w, h, px))
    }
}

fn begin<'e>(enc: &'e mut wgpu::CommandEncoder, view: &wgpu::TextureView, clear: Option<wgpu::Color>) -> wgpu::RenderPass<'e> {
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations { load: clear.map_or(wgpu::LoadOp::Load, wgpu::LoadOp::Clear), store: wgpu::StoreOp::Store },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// The four corners of a layer, cropped and moved, in clip space with their
/// texture coordinates: top-left, top-right, bottom-left, bottom-right.
pub fn quad(m: &Motion, src: (f32, f32), seq: (f32, f32)) -> [[f32; 4]; 4] {
    let [cl, ct, cr, cb] = m.crop.map(|c| (c / 100.0).clamp(0.0, 1.0));
    let (u0, v0, u1, v1) = (cl, ct, (1.0 - cr).max(cl), (1.0 - cb).max(ct));
    let s = m.scale / 100.0;
    let (sin, cos) = m.rotation.to_radians().sin_cos();
    [(u0, v0), (u1, v0), (u0, v1), (u1, v1)].map(|(u, v)| {
        let (x, y) = (u * src.0 - m.anchor[0], v * src.1 - m.anchor[1]);
        let (x, y) = (x * s, y * s);
        let (x, y) = (x * cos - y * sin + m.position[0], x * sin + y * cos + m.position[1]);
        [x / seq.0 * 2.0 - 1.0, 1.0 - y / seq.1 * 2.0, u, v]
    })
}
