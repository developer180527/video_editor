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

use ve_ports::{CpuPlanes, FrameData, PixelFormat, VideoFrame};

use crate::FramePlan;

/// The engine's working format: half-float RGBA, linear light.
pub const WORKING_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// What the monitor shows: display-encoded RGBA8.
pub const DISPLAY_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Turns a frame left in GPU memory by a hardware decoder into textures on
/// the compositor's device without a copy: a `CVPixelBuffer` → Metal
/// textures on Apple (unified memory, or a discrete GPU's VRAM via IOSurface).
/// Platforms supply one; frames it cannot import take the CPU path.
pub trait TextureImporter: Send + Sync {
    /// The frame's luma and chroma planes: `R8Unorm` + `Rg8Unorm` for NV12,
    /// `R16Unorm` + `Rg16Unorm` for P010 (only on devices with
    /// `TEXTURE_FORMAT_16BIT_NORM`). `None` if this frame is not one it reads.
    fn import(&self, device: &wgpu::Device, frame: &VideoFrame) -> Option<[wgpu::Texture; 2]>;
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
    // Columns of the source-primaries → working-space matrix; p0.w is the
    // chroma midpoint as the texture reads it.
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
    let c = (textureSample(chroma, samp, i.uv).rg - vec2<f32>(d.p0.w, d.p0.w)) * d.range.z;
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

const MIP: &str = r#"
@group(0) @binding(1) var above: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

@fragment
fn fs_mip(i: VsOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(above, samp, i.uv, 0.0);
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

/// Luma offset, luma scale, chroma scale and chroma midpoint that turn
/// normalised texture values into Y' in 0..1 and Cb/Cr in -0.5..0.5.
/// `deep`: P010 in a 16-bit texture, i.e. a 10-bit value `v` reads as
/// `v·64/65535` (it sits in the top bits), not `v/1023`. Otherwise 8-bit,
/// reading `v/255`, whose midpoint is 128/255 — not 0.5.
pub fn yuv_levels(deep: bool, full_range: bool) -> (f32, f32, f32, f32) {
    if deep {
        let q = 64.0 / 65535.0; // one 10-bit step, as the texture reads it
        if full_range {
            (0.0, 1.0 / (1023.0 * q), 1.0 / (1023.0 * q), 512.0 * q)
        } else {
            (64.0 * q, 1.0 / (876.0 * q), 1.0 / (896.0 * q), 512.0 * q)
        }
    } else if full_range {
        (0.0, 1.0, 1.0, 128.0 / 255.0)
    } else {
        (16.0 / 255.0, 255.0 / 219.0, 255.0 / 224.0, 128.0 / 255.0)
    }
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

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

struct Planes {
    luma: wgpu::Texture,
    chroma: wgpu::Texture,
    luma_view: wgpu::TextureView,
    chroma_view: wgpu::TextureView,
    key: (u32, u32, bool),
    /// Imported from GPU memory (not ours to write into).
    imported: bool,
    /// The frame these textures hold, so an unchanged frame is not
    /// re-uploaded. Held (not just its address) so it cannot be freed and
    /// another frame take its place unnoticed.
    holds: Option<Arc<VideoFrame>>,
}

/// A texture that keeps its size between frames (the working target, the
/// monitor), with its view.
struct Target {
    tex: wgpu::Texture,
    view: wgpu::TextureView,
    w: u32,
    h: u32,
}

/// One texture of a layer's ping-pong pair, with its mip chain.
struct LayerTex {
    /// Level 0 alone: what passes render into and effects read.
    base: wgpu::TextureView,
    /// Every level: what the composite samples.
    all: wgpu::TextureView,
    /// Levels 1.. alone: the targets of the mip passes.
    levels: Vec<wgpu::TextureView>,
}

/// Everything one layer position keeps between frames, so a steady frame
/// creates no GPU objects: textures, views, uniform buffers (rewritten in
/// place) and bind groups (rebuilt only when their textures change).
struct Slot {
    planes: Option<Planes>,
    pair: Option<[LayerTex; 2]>,
    /// Width, height and mip levels of `pair`.
    pair_key: (u32, u32, u32),
    decode_buf: wgpu::Buffer,
    quad_buf: wgpu::Buffer,
    effect_bufs: Vec<wgpu::Buffer>,
    decode_bg: Option<wgpu::BindGroup>,
    /// By which texture of the pair is composited.
    quad_bg: [Option<wgpu::BindGroup>; 2],
    /// By effect index, then by source texture.
    effect_bg: Vec<[Option<wgpu::BindGroup>; 2]>,
    /// By texture, then by level (reading the level above).
    mip_bg: [Vec<wgpu::BindGroup>; 2],
}

const DECODE_BYTES: u64 = 7 * 16;
const QUAD_BYTES: u64 = 5 * 16;
const EFFECT_BYTES: u64 = (64 * 4 + 4) * 4;
const DISPLAY_BYTES: u64 = 3 * 16;

fn uniform_buf(device: &wgpu::Device, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("uniform"),
        size,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl Slot {
    fn new(device: &wgpu::Device) -> Self {
        Slot {
            planes: None,
            pair: None,
            pair_key: (0, 0, 0),
            decode_buf: uniform_buf(device, DECODE_BYTES),
            quad_buf: uniform_buf(device, QUAD_BYTES),
            effect_bufs: Vec::new(),
            decode_bg: None,
            quad_bg: [None, None],
            effect_bg: Vec::new(),
            mip_bg: [Vec::new(), Vec::new()],
        }
    }
}

/// Mip levels worth having for a layer drawn at `scale` (target pixels per
/// source pixel): enough that the composite never minifies by more than 2×.
fn mip_levels(scale: f32, w: u32, h: u32) -> u32 {
    let max = 32 - w.max(h).max(1).leading_zeros();
    if !(scale > 0.0 && scale < 0.5) {
        return 1;
    }
    ((1.0 / scale).log2().floor() as u32 + 1).min(max)
}

pub struct Compositor {
    sixteen_bit: bool,
    sampler: wgpu::Sampler,
    decode: wgpu::RenderPipeline,
    decode_layout: wgpu::BindGroupLayout,
    one_layout: wgpu::BindGroupLayout,
    composite: HashMap<Blend, wgpu::RenderPipeline>,
    display: wgpu::RenderPipeline,
    mip: wgpu::RenderPipeline,
    effects: HashMap<String, Result<wgpu::RenderPipeline, String>>,
    effect_layout: wgpu::BindGroupLayout,
    work: Option<Target>,
    out: Option<Target>,
    slots: Vec<Slot>,
    display_buf: wgpu::Buffer,
    display_bg: Option<wgpu::BindGroup>,
    /// Bound where a pass takes a uniform it does not use (mip passes).
    empty_buf: wgpu::Buffer,
    dummy_view: wgpu::TextureView,
    /// Readback buffers returned by finished [`Readback`]s, for reuse: on a
    /// GPU with its own memory each is a host-visible allocation.
    readback_pool: Arc<std::sync::Mutex<Vec<wgpu::Buffer>>>,
    importer: Option<Arc<dyn TextureImporter>>,
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
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
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
        let mip = pipeline(device, &format!("{COMMON}{MIP}"), "vs_full", "fs_mip", &one_layout, WORKING_FORMAT, None);
        let dummy = texture(device, "dummy", 1, 1, WORKING_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING);
        Compositor {
            sixteen_bit,
            sampler,
            decode,
            decode_layout,
            one_layout,
            composite,
            display,
            mip,
            effects: HashMap::new(),
            effect_layout,
            work: None,
            out: None,
            slots: Vec::new(),
            display_buf: uniform_buf(device, DISPLAY_BYTES),
            display_bg: None,
            empty_buf: uniform_buf(device, 16),
            dummy_view: dummy.create_view(&Default::default()),
            readback_pool: Default::default(),
            importer: None,
            errors: Vec::new(),
        }
    }

    /// Use `importer` for frames left in GPU memory by hardware decoders.
    pub fn set_importer(&mut self, importer: Option<Arc<dyn TextureImporter>>) {
        self.importer = importer;
    }

    /// The monitor texture (display-encoded RGBA8) from the last render.
    pub fn output(&self) -> Option<&wgpu::Texture> {
        self.out.as_ref().map(|s| &s.tex)
    }

    /// Make `slot` hold a `w`×`h` texture; true if it was (re)made.
    fn target(slot: &mut Option<Target>, device: &wgpu::Device, label: &str, w: u32, h: u32, format: wgpu::TextureFormat, usage: wgpu::TextureUsages) -> bool {
        if matches!(slot, Some(s) if s.w == w && s.h == h) {
            return false;
        }
        let tex = texture(device, label, w, h, format, usage);
        *slot = Some(Target { view: tex.create_view(&Default::default()), tex, w, h });
        true
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

    /// Upload one frame's planes into slot `i`. False if the frame is not in
    /// memory (nothing to draw).
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, i: usize, frame: &Arc<VideoFrame>) -> bool {
        let f: &VideoFrame = frame;
        let slot = &mut self.slots[i];
        // The same frame as last time (video slower than the display): done.
        if slot.planes.as_ref().is_some_and(|p| p.holds.as_ref().is_some_and(|h| Arc::ptr_eq(h, frame))) {
            return true;
        }
        // In GPU memory: import it as it is, if the platform can.
        if matches!(f.data, FrameData::Native(_)) {
            if let Some([luma, chroma]) = self.importer.as_ref().and_then(|imp| imp.import(device, f)) {
                let deep = luma.format() == wgpu::TextureFormat::R16Unorm;
                slot.planes = Some(Planes {
                    luma_view: luma.create_view(&Default::default()),
                    chroma_view: chroma.create_view(&Default::default()),
                    luma,
                    chroma,
                    key: (f.width, f.height, deep),
                    imported: true,
                    holds: Some(frame.clone()),
                });
                slot.decode_bg = None;
                return true;
            }
        }
        // In memory (or copied out of GPU memory): upload.
        let Some(CpuPlanes { planes, strides }) = f.data.cpu() else { return false };
        let deep = f.format == PixelFormat::P010 && self.sixteen_bit;
        let key = (f.width, f.height, deep);
        if slot.planes.as_ref().is_none_or(|p| p.key != key || p.imported) {
            let (lf, cf) = if deep {
                (wgpu::TextureFormat::R16Unorm, wgpu::TextureFormat::Rg16Unorm)
            } else {
                (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm)
            };
            let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
            let luma = texture(device, "luma", f.width, f.height, lf, usage);
            let chroma = texture(device, "chroma", f.width.div_ceil(2), f.height.div_ceil(2), cf, usage);
            slot.planes = Some(Planes { luma_view: luma.create_view(&Default::default()), chroma_view: chroma.create_view(&Default::default()), luma, chroma, key, imported: false, holds: None });
            slot.decode_bg = None;
        }
        let p = slot.planes.as_mut().unwrap();
        p.holds = Some(frame.clone());
        // 10-bit without 16-bit textures: keep the top 8 bits.
        let squash = |plane: &[u8]| -> Vec<u8> { plane.as_chunks::<2>().0.iter().map(|c| c[1]).collect() };
        // Borrowed straight from the frame unless it has to be narrowed.
        let (luma, chroma, ls, cs): (std::borrow::Cow<[u8]>, std::borrow::Cow<[u8]>, usize, usize) = if f.format == PixelFormat::P010 && !deep {
            (squash(&planes[0]).into(), squash(&planes[1]).into(), strides[0] / 2, strides[1] / 2)
        } else {
            let mut p = planes.into_iter();
            (p.next().unwrap(), p.next().unwrap(), strides[0], strides[1])
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

    /// Make slot `i`'s layer pair `w`×`h` with `levels` mip levels.
    fn layer_pair(&mut self, device: &wgpu::Device, i: usize, w: u32, h: u32, levels: u32) {
        let slot = &mut self.slots[i];
        if slot.pair.is_some() && slot.pair_key == (w, h, levels) {
            return;
        }
        let make = || {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("layer"),
                size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
                mip_level_count: levels,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: WORKING_FORMAT,
                usage: LAYER_USAGE,
                view_formats: &[],
            });
            let level = |l: u32| tex.create_view(&wgpu::TextureViewDescriptor { base_mip_level: l, mip_level_count: Some(1), ..Default::default() });
            LayerTex { base: level(0), all: tex.create_view(&Default::default()), levels: (1..levels).map(level).collect() }
        };
        slot.pair = Some([make(), make()]);
        slot.pair_key = (w, h, levels);
        slot.quad_bg = [None, None];
        slot.effect_bg.iter_mut().for_each(|b| *b = [None, None]);
        slot.mip_bg = [Vec::new(), Vec::new()];
    }

    /// Draw `layers` (bottom first) for `plan` and produce the monitor image.
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, plan: &FramePlan, layers: &[RenderLayer], seq_size: (u32, u32), space: WorkingSpace) {
        let (w, h) = (plan.width, plan.height);
        let target_usage = LAYER_USAGE | wgpu::TextureUsages::COPY_SRC;
        if Self::target(&mut self.work, device, "work", w, h, WORKING_FORMAT, target_usage) {
            self.display_bg = None;
        }
        Self::target(&mut self.out, device, "monitor", w, h, DISPLAY_FORMAT, target_usage);
        while self.slots.len() < layers.len() {
            self.slots.push(Slot::new(device));
        }
        let mut enc = device.create_command_encoder(&Default::default());
        {
            let _clear = begin(&mut enc, &self.work.as_ref().unwrap().view, Some(wgpu::Color::TRANSPARENT));
        }
        for (i, layer) in layers.iter().enumerate() {
            let f = &layer.frame;
            if !self.upload(device, queue, i, f) {
                continue;
            }
            // Target pixels per source pixel decides the mip chain.
            let scale = layer.motion.scale / 100.0 * plan.width as f32 / seq_size.0.max(1) as f32;
            let levels = mip_levels(scale, f.width, f.height);
            self.layer_pair(device, i, f.width, f.height, levels);

            // 1. Decode.
            let m = columns(ycbcr(&f.color.matrix, f.height));
            // How the planes in the textures read (16-bit textures hold
            // P010 at full depth; otherwise everything is 8-bit).
            let deep = self.slots[i].planes.as_ref().is_some_and(|p| p.key.2);
            let (yo, ys, cs, cm) = yuv_levels(deep, f.color.full_range);
            let mut pm = columns(primaries_to(space, &f.color.primaries));
            pm[0][3] = cm; // the chroma midpoint rides in the spare lane
            let mut u = Vec::with_capacity(28);
            for c in m {
                u.extend(c);
            }
            u.extend([yo, ys, cs, transfer_id(&f.color.transfer)]);
            for c in pm {
                u.extend(c);
            }
            queue.write_buffer(&self.slots[i].decode_buf, 0, &floats(&u));
            if self.slots[i].decode_bg.is_none() {
                let s = &self.slots[i];
                let p = s.planes.as_ref().unwrap();
                let bg = self.bind(device, &self.decode_layout, &s.decode_buf, &[&p.luma_view, &p.chroma_view]);
                self.slots[i].decode_bg = Some(bg);
            }
            let mut cur = 0;
            {
                let s = &self.slots[i];
                let mut pass = begin(&mut enc, &s.pair.as_ref().unwrap()[cur].base, Some(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(&self.decode);
                pass.set_bind_group(0, s.decode_bg.as_ref(), &[]);
                pass.draw(0..3, 0..1);
            }

            // 2. Effects.
            for (k, e) in layer.effects.iter().enumerate() {
                if self.effect_pipeline(device, e).is_none() {
                    continue;
                }
                let mut vals = vec![0f32; 64 * 4 + 4];
                for (n, p) in e.params.iter().take(64).enumerate() {
                    vals[n * 4..n * 4 + 4].copy_from_slice(p);
                }
                vals[256] = e.time;
                vals[258] = plan.width as f32 / seq_size.0.max(1) as f32;
                let s = &mut self.slots[i];
                while s.effect_bufs.len() <= k {
                    s.effect_bufs.push(uniform_buf(device, EFFECT_BYTES));
                    s.effect_bg.push([None, None]);
                }
                queue.write_buffer(&s.effect_bufs[k], 0, &floats(&vals));
                if self.slots[i].effect_bg[k][cur].is_none() {
                    let s = &self.slots[i];
                    // Bindings: source, (unused slot), source_b.
                    let src = &s.pair.as_ref().unwrap()[cur].base;
                    let bg = self.bind(device, &self.effect_layout, &s.effect_bufs[k], &[src, &self.dummy_view, &self.dummy_view]);
                    self.slots[i].effect_bg[k][cur] = Some(bg);
                }
                let s = &self.slots[i];
                let next = cur ^ 1;
                let pipe = self.effects[&e.key].as_ref().unwrap();
                let mut pass = begin(&mut enc, &s.pair.as_ref().unwrap()[next].base, Some(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(pipe);
                pass.set_bind_group(0, s.effect_bg[k][cur].as_ref(), &[]);
                pass.draw(0..3, 0..1);
                drop(pass);
                cur = next;
            }

            // 3. Mip chain, when the layer is drawn below half size: each
            // level a 2×2 average of the one above (bilinear at its centre).
            if levels > 1 {
                if self.slots[i].mip_bg[cur].is_empty() {
                    let s = &self.slots[i];
                    let t = &s.pair.as_ref().unwrap()[cur];
                    let bgs = (1..levels as usize)
                        .map(|l| {
                            let above = if l == 1 { &t.base } else { &t.levels[l - 2] };
                            self.bind(device, &self.one_layout, &self.empty_buf, &[above])
                        })
                        .collect();
                    self.slots[i].mip_bg[cur] = bgs;
                }
                let s = &self.slots[i];
                let t = &s.pair.as_ref().unwrap()[cur];
                for (l, bg) in s.mip_bg[cur].iter().enumerate() {
                    let mut pass = begin(&mut enc, &t.levels[l], Some(wgpu::Color::TRANSPARENT));
                    pass.set_pipeline(&self.mip);
                    pass.set_bind_group(0, bg, &[]);
                    pass.draw(0..3, 0..1);
                }
            }

            // 4. Composite.
            let corners = quad(&layer.motion, (f.width as f32, f.height as f32), (seq_size.0 as f32, seq_size.1 as f32));
            let mut u: Vec<f32> = corners.iter().flatten().copied().collect();
            u.extend([layer.opacity.clamp(0.0, 1.0), 0.0, 0.0, 0.0]);
            queue.write_buffer(&self.slots[i].quad_buf, 0, &floats(&u));
            if self.slots[i].quad_bg[cur].is_none() {
                let s = &self.slots[i];
                let bg = self.bind(device, &self.one_layout, &s.quad_buf, &[&s.pair.as_ref().unwrap()[cur].all]);
                self.slots[i].quad_bg[cur] = Some(bg);
            }
            let s = &self.slots[i];
            let mut pass = begin(&mut enc, &self.work.as_ref().unwrap().view, None);
            pass.set_pipeline(&self.composite[&layer.blend]);
            pass.set_bind_group(0, s.quad_bg[cur].as_ref(), &[]);
            pass.draw(0..6, 0..1);
        }
        // Display.
        let to_display = match space {
            WorkingSpace::AcesCg => AP1_TO_709,
            WorkingSpace::LinearRec709 => IDENTITY,
        };
        let u: Vec<f32> = columns(to_display).iter().flatten().copied().collect();
        queue.write_buffer(&self.display_buf, 0, &floats(&u));
        if self.display_bg.is_none() {
            self.display_bg = Some(self.bind(device, &self.one_layout, &self.display_buf, &[&self.work.as_ref().unwrap().view]));
        }
        {
            let mut pass = begin(&mut enc, &self.out.as_ref().unwrap().view, Some(wgpu::Color::BLACK));
            pass.set_pipeline(&self.display);
            pass.set_bind_group(0, self.display_bg.as_ref(), &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([enc.finish()]);
    }

    /// Copy the monitor image back as RGBA8 rows (export, tests). Waits.
    pub fn read_output(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<(u32, u32, Vec<u8>)> {
        self.start_readback(device, queue)?.finish(device)
    }

    /// Start copying the monitor image back without waiting for it, so the
    /// next frame can be rendered meanwhile; [`Readback::finish`] collects
    /// it. Later renders do not disturb it: the copy is queued first.
    pub fn start_readback(&self, device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Readback> {
        let out = self.out.as_ref()?;
        let (w, h) = (out.w, out.h);
        let row = (w * 4).div_ceil(256) * 256;
        let size = (row * h) as u64;
        let pooled = {
            let mut pool = self.readback_pool.lock().unwrap();
            pool.retain(|b| b.size() == size); // a new output size: old ones go
            pool.pop()
        };
        let buf = pooled.unwrap_or_else(|| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            })
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &out.tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo { buffer: &buf, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let index = queue.submit([enc.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        Some(Readback { buf, w, h, row, index, rx, pool: self.readback_pool.clone() })
    }
}

/// A monitor image on its way back from the GPU.
pub struct Readback {
    buf: wgpu::Buffer,
    w: u32,
    h: u32,
    row: u32,
    index: wgpu::SubmissionIndex,
    rx: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    pool: Arc<std::sync::Mutex<Vec<wgpu::Buffer>>>,
}

impl Readback {
    /// Wait for this copy (only this one, not work queued after it) and
    /// return tightly packed RGBA8 rows.
    pub fn finish(self, device: &wgpu::Device) -> Option<(u32, u32, Vec<u8>)> {
        let _ = device.poll(wgpu::PollType::Wait { submission_index: Some(self.index), timeout: None });
        self.rx.recv().ok()?.ok()?;
        let (w, h, row) = (self.w as usize, self.h as usize, self.row as usize);
        let data = self.buf.slice(..).get_mapped_range().ok()?;
        let mut px = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            px.extend_from_slice(&data[y * row..y * row + w * 4]);
        }
        drop(data);
        self.buf.unmap();
        let mut pool = self.pool.lock().unwrap();
        if pool.len() < 4 {
            pool.push(self.buf);
        }
        Some((self.w, self.h, px))
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
