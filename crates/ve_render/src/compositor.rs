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
/// The deep output (export): the same display-encoded picture as 16-bit
/// integers, 0..65535. Renderable on every GPU, unlike 16-bit normalized.
pub const DEEP_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Uint;

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
    pub source: LayerSource,
    /// The picture's size as Motion sees it, in its own pixels: a proxy
    /// layer has its original's size, so positions and anchors still fit.
    pub size: (u32, u32),
    pub motion: Motion,
    /// 0..1.
    pub opacity: f32,
    pub blend: Blend,
    pub effects: Vec<GpuEffect>,
    /// Inside a transition: the other layer and the transition's shader.
    pub transition: Option<Box<RenderTransition>>,
    /// Degrees the stored picture turns clockwise to stand upright (a phone
    /// held upright records landscape): 0, 90, 180 or 270. `size` is the
    /// upright size; effects see the picture as stored.
    pub turn: u16,
}

impl RenderLayer {
    /// A plain layer showing `frame` at its own size.
    pub fn of_frame(frame: Arc<VideoFrame>, motion: Motion, opacity: f32) -> Self {
        RenderLayer { size: (frame.width, frame.height), source: LayerSource::Frame(frame), motion, opacity, blend: Blend::Normal, effects: vec![], transition: None, turn: 0 }
    }
}

/// Where a layer's picture comes from.
#[derive(Clone)]
pub enum LayerSource {
    /// A decoded picture: YCbCr from a decoder, or RGBA (display-referred,
    /// straight alpha) from a generator.
    Frame(Arc<VideoFrame>),
    /// A plugin generator's shader, drawn at the layer's size.
    Shader(GpuEffect),
    /// A nested sequence, composited (scene-linear) at its own size.
    Nested(Box<NestedLayers>),
}

/// A nested sequence's layers, drawn into one picture.
#[derive(Clone)]
pub struct NestedLayers {
    /// The picture's size in pixels (the nested format at this quality).
    pub width: u32,
    pub height: u32,
    /// The nested sequence's own format size: its layers' Motion space.
    pub seq_size: (u32, u32),
    pub layers: Vec<RenderLayer>,
}

/// A layer's transition: both pictures are drawn, then mixed by `effect`
/// (`source` going out, `source_b` coming in, `progress` 0..1).
#[derive(Clone)]
pub struct RenderTransition {
    pub effect: GpuEffect,
    pub progress: f32,
    /// The layer coming in; `None` for a fade to or from nothing.
    pub incoming: Option<RenderLayer>,
    /// The layer itself is the one coming in (a fade from nothing).
    pub self_incoming: bool,
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

/// The decode uniform: matrix, levels and transfer, primaries.
const DECODE_STRUCT: &str = r#"
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
"#;

/// Transfer functions to scene-linear, and the HDR roll-off.
const TRANSFER: &str = r#"
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

"#;

/// YCbCr planes → working space.
const DECODE: &str = r#"
@group(0) @binding(0) var<uniform> d: Decode;
@group(0) @binding(1) var luma: texture_2d<f32>;
@group(0) @binding(2) var chroma: texture_2d<f32>;
@group(0) @binding(3) var samp: sampler;

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

/// RGBA (display-referred, straight alpha: generators, titles, stills) →
/// premultiplied working space, through the same transfer and primaries.
const RGBA_DECODE: &str = r#"
@group(0) @binding(0) var<uniform> d: Decode;
@group(0) @binding(1) var rgba: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

@fragment
fn fs_rgba(i: VsOut) -> @location(0) vec4<f32> {
    let c = textureSample(rgba, samp, i.uv);
    let p = mat3x3<f32>(d.p0.xyz, d.p1.xyz, d.p2.xyz);
    let lin = p * to_linear(c.rgb, d.range.w);
    return vec4<f32>(lin * c.a, c.a);
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

// The monitor signal: Rec.709 primaries, BT.1886 (2.4), over black.
fn display_signal(uv: vec2<f32>) -> vec4<f32> {
    let c = textureSample(work, samp, uv);
    let m = mat3x3<f32>(d.w0.xyz, d.w1.xyz, d.w2.xyz);
    let rgb = clamp(m * c.rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(pow(rgb, vec3<f32>(1.0 / 2.4)), 1.0);
}

@fragment
fn fs_display(i: VsOut) -> @location(0) vec4<f32> {
    return display_signal(i.uv);
}

// The same signal at 16 bits, for export: exact integers, no 8-bit step.
@fragment
fn fs_display_deep(i: VsOut) -> @location(0) vec4<u32> {
    let v = display_signal(i.uv);
    return vec4<u32>(round(clamp(v, vec4<f32>(0.0), vec4<f32>(1.0)) * 65535.0));
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

/// The host's vertex stage for effects, after the frozen prelude
/// (`ve_plugin_abi::WGSL_PRELUDE`, the contract plugins are written to).
const EFFECT_VERTEX: &str = r#"
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
// (AP1 → 709 is computed as the exact inverse of 709 → AP1: the published
// pair are each rounded, so 709 colours would not come back exact.)
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

/// The inverse of a 3×3 matrix (in f64, so a round trip is exact in f32).
fn inverse(m: M3) -> M3 {
    let a = m.map(|r| r.map(|x| x as f64));
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1]) - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0]) + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    let c = |r: usize, k: usize| {
        let (r1, r2, k1, k2) = ((r + 1) % 3, (r + 2) % 3, (k + 1) % 3, (k + 2) % 3);
        a[r1][k1] * a[r2][k2] - a[r1][k2] * a[r2][k1]
    };
    // Transposed cofactors over the determinant.
    [0, 1, 2].map(|i| [0, 1, 2].map(|j| (c(j, i) / det) as f32))
}

fn ap1_to_709() -> M3 {
    inverse(REC709_TO_AP1)
}

/// A colour parameter as stored (display-encoded: what the colour picker
/// and the monitor show, straight alpha) to what a shader works in: linear
/// light in the working space, straight alpha. The exact inverse of the
/// monitor's encoding (working → Rec.709 → γ 2.4), so a picked colour
/// comes back on the monitor as the same value.
pub fn display_color_to_working(c: [f32; 4], space: WorkingSpace) -> [f32; 4] {
    let lin = [0, 1, 2].map(|k| c[k].max(0.0).powf(2.4));
    let m = primaries_to(space, "bt709");
    let w = [0, 1, 2].map(|r| m[r][0] * lin[0] + m[r][1] * lin[1] + m[r][2] * lin[2]);
    [w[0], w[1], w[2], c[3]]
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
    /// One RGBA8 texture (in `luma`) rather than YCbCr planes.
    rgba: bool,
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
    /// A transition's pictures going out and coming in (with mip chains,
    /// which its shader may read), their size and levels, and their mix.
    trans: Option<[LayerTex; 2]>,
    trans_key: (u32, u32, u32),
    trans_mip_bg: [Vec<wgpu::BindGroup>; 2],
    trans_mix: Option<Target>,
    trans_buf: wgpu::Buffer,
    /// The whole-frame quad that lays a transition's mix down.
    ident_buf: wgpu::Buffer,
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
            trans: None,
            trans_key: (0, 0, 0),
            trans_mip_bg: [Vec::new(), Vec::new()],
            trans_mix: None,
            trans_buf: uniform_buf(device, EFFECT_BYTES),
            ident_buf: uniform_buf(device, QUAD_BYTES),
        }
    }
}

/// Mip levels worth having for a layer drawn at `scale` (target pixels per
/// source pixel): enough that the composite never minifies by more than 2×.
fn mip_levels(scale: f32, w: u32, h: u32) -> u32 {
    let max = full_levels(w, h);
    if !(scale > 0.0 && scale < 0.5) {
        return 1;
    }
    ((1.0 / scale).log2().floor() as u32 + 1).min(max)
}

/// Every mip level down to 1×1.
fn full_levels(w: u32, h: u32) -> u32 {
    32 - w.max(h).max(1).leading_zeros()
}

/// A working-format texture of `levels` mip levels, with its views.
fn layer_tex(device: &wgpu::Device, w: u32, h: u32, levels: u32) -> LayerTex {
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
}

pub struct Compositor {
    sixteen_bit: bool,
    sampler: wgpu::Sampler,
    decode: wgpu::RenderPipeline,
    decode_rgba: wgpu::RenderPipeline,
    decode_layout: wgpu::BindGroupLayout,
    one_layout: wgpu::BindGroupLayout,
    composite: HashMap<Blend, wgpu::RenderPipeline>,
    display: wgpu::RenderPipeline,
    display_deep: wgpu::RenderPipeline,
    /// Render the output at 16 bits ([`DEEP_FORMAT`]) instead of 8.
    deep_output: bool,
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
        let decode = pipeline(device, &format!("{COMMON}{DECODE_STRUCT}{TRANSFER}{DECODE}"), "vs_full", "fs_decode", &decode_layout, WORKING_FORMAT, None);
        let decode_rgba = pipeline(device, &format!("{COMMON}{DECODE_STRUCT}{TRANSFER}{RGBA_DECODE}"), "vs_full", "fs_rgba", &one_layout, WORKING_FORMAT, None);
        let composite = [Blend::Normal, Blend::Multiply, Blend::Screen, Blend::Add, Blend::Overlay]
            .into_iter()
            .map(|b| (b, pipeline(device, COMPOSITE, "vs_quad", "fs_quad", &one_layout, WORKING_FORMAT, Some(blend_state(b)))))
            .collect();
        let display = pipeline(device, &format!("{COMMON}{DISPLAY}"), "vs_full", "fs_display", &one_layout, DISPLAY_FORMAT, None);
        let display_deep = pipeline(device, &format!("{COMMON}{DISPLAY}"), "vs_full", "fs_display_deep", &one_layout, DEEP_FORMAT, None);
        let mip = pipeline(device, &format!("{COMMON}{MIP}"), "vs_full", "fs_mip", &one_layout, WORKING_FORMAT, None);
        let dummy = texture(device, "dummy", 1, 1, WORKING_FORMAT, wgpu::TextureUsages::TEXTURE_BINDING);
        Compositor {
            sixteen_bit,
            sampler,
            decode,
            decode_rgba,
            decode_layout,
            one_layout,
            composite,
            display,
            display_deep,
            deep_output: false,
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

    /// Render the output at 16 bits per channel ([`DEEP_FORMAT`]): for
    /// export, so 10-bit codecs get 10 real bits and 8-bit ones are rounded
    /// from the full picture. The monitor stays 8-bit.
    pub fn set_deep_output(&mut self, deep: bool) {
        self.deep_output = deep;
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
        if matches!(slot, Some(s) if s.w == w && s.h == h && s.tex.format() == format) {
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
            let src = format!("{}{EFFECT_VERTEX}\n{}", ve_plugin_abi::WGSL_PRELUDE, e.wgsl);
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
                    rgba: false,
                    holds: Some(frame.clone()),
                });
                slot.decode_bg = None;
                return true;
            }
        }
        // In memory (or copied out of GPU memory): upload.
        let Some(CpuPlanes { planes, strides }) = f.data.cpu() else { return false };
        if f.format == PixelFormat::Rgba8 {
            let key = (f.width, f.height, false);
            if slot.planes.as_ref().is_none_or(|p| p.key != key || !p.rgba) {
                let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
                let tex = texture(device, "rgba", f.width, f.height, wgpu::TextureFormat::Rgba8Unorm, usage);
                let none = texture(device, "unused", 1, 1, wgpu::TextureFormat::Rg8Unorm, usage);
                slot.planes = Some(Planes { luma_view: tex.create_view(&Default::default()), chroma_view: none.create_view(&Default::default()), luma: tex, chroma: none, key, imported: false, rgba: true, holds: None });
                slot.decode_bg = None;
            }
            let p = slot.planes.as_mut().unwrap();
            p.holds = Some(frame.clone());
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &p.luma, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                &planes[0],
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(strides[0] as u32), rows_per_image: Some(f.height) },
                wgpu::Extent3d { width: f.width, height: f.height, depth_or_array_layers: 1 },
            );
            return true;
        }
        let deep = f.format == PixelFormat::P010 && self.sixteen_bit;
        let key = (f.width, f.height, deep);
        if slot.planes.as_ref().is_none_or(|p| p.key != key || p.imported || p.rgba) {
            let (lf, cf) = if deep {
                (wgpu::TextureFormat::R16Unorm, wgpu::TextureFormat::Rg16Unorm)
            } else {
                (wgpu::TextureFormat::R8Unorm, wgpu::TextureFormat::Rg8Unorm)
            };
            let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
            let luma = texture(device, "luma", f.width, f.height, lf, usage);
            let chroma = texture(device, "chroma", f.width.div_ceil(2), f.height.div_ceil(2), cf, usage);
            slot.planes = Some(Planes { luma_view: luma.create_view(&Default::default()), chroma_view: chroma.create_view(&Default::default()), luma, chroma, key, imported: false, rgba: false, holds: None });
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
        slot.pair = Some([layer_tex(device, w, h, levels), layer_tex(device, w, h, levels)]);
        slot.pair_key = (w, h, levels);
        slot.quad_bg = [None, None];
        slot.effect_bg.iter_mut().for_each(|b| *b = [None, None]);
        slot.mip_bg = [Vec::new(), Vec::new()];
    }

    /// Bind groups for `t`'s mip passes: each level reads the one above.
    fn mip_groups(&self, device: &wgpu::Device, t: &LayerTex) -> Vec<wgpu::BindGroup> {
        (1..=t.levels.len()).map(|l| self.bind(device, &self.one_layout, &self.empty_buf, &[if l == 1 { &t.base } else { &t.levels[l - 2] }])).collect()
    }

    /// Fill `t`'s levels 1.., each a 2×2 average of the level above
    /// (bilinear at its centre).
    fn mip_passes(&self, enc: &mut wgpu::CommandEncoder, t: &LayerTex, groups: &[wgpu::BindGroup]) {
        for (l, bg) in groups.iter().enumerate() {
            let mut pass = begin(enc, &t.levels[l], Some(wgpu::Color::TRANSPARENT));
            pass.set_pipeline(&self.mip);
            pass.set_bind_group(0, bg, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Fill the mip chain of slot `i`'s texture `cur` (bind groups made
    /// once, kept with the slot).
    fn layer_mips(&mut self, device: &wgpu::Device, enc: &mut wgpu::CommandEncoder, i: usize, cur: usize) {
        if self.slots[i].mip_bg[cur].is_empty() {
            let bgs = self.mip_groups(device, &self.slots[i].pair.as_ref().unwrap()[cur]);
            self.slots[i].mip_bg[cur] = bgs;
        }
        let s = &self.slots[i];
        self.mip_passes(enc, &s.pair.as_ref().unwrap()[cur], &s.mip_bg[cur]);
    }

    /// Draw `layers` (bottom first) for `plan` and produce the monitor image.
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, plan: &FramePlan, layers: &[RenderLayer], seq_size: (u32, u32), space: WorkingSpace) {
        let (w, h) = (plan.width, plan.height);
        let target_usage = LAYER_USAGE | wgpu::TextureUsages::COPY_SRC;
        if Self::target(&mut self.work, device, "work", w, h, WORKING_FORMAT, target_usage) {
            self.display_bg = None;
        }
        let out_format = if self.deep_output { DEEP_FORMAT } else { DISPLAY_FORMAT };
        Self::target(&mut self.out, device, "monitor", w, h, out_format, target_usage);
        let mut enc = device.create_command_encoder(&Default::default());
        let work = self.work.as_ref().unwrap().view.clone();
        {
            let _clear = begin(&mut enc, &work, Some(wgpu::Color::TRANSPARENT));
        }
        let mut next = 0;
        self.draw(device, queue, &mut enc, &work, (w, h), layers, seq_size, space, &mut next);
        // Display.
        let to_display = match space {
            WorkingSpace::AcesCg => ap1_to_709(),
            WorkingSpace::LinearRec709 => IDENTITY,
        };
        let u: Vec<f32> = columns(to_display).iter().flatten().copied().collect();
        queue.write_buffer(&self.display_buf, 0, &floats(&u));
        if self.display_bg.is_none() {
            self.display_bg = Some(self.bind(device, &self.one_layout, &self.display_buf, &[&self.work.as_ref().unwrap().view]));
        }
        {
            let mut pass = begin(&mut enc, &self.out.as_ref().unwrap().view, Some(wgpu::Color::BLACK));
            pass.set_pipeline(if self.deep_output { &self.display_deep } else { &self.display });
            pass.set_bind_group(0, self.display_bg.as_ref(), &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit([enc.finish()]);
    }

    /// The next free slot this frame. Layers take slots in drawing order —
    /// nested and transitioning ones too — so a steady frame reuses them.
    fn slot(&mut self, device: &wgpu::Device, next: &mut usize) -> usize {
        let i = *next;
        *next += 1;
        while self.slots.len() <= i {
            self.slots.push(Slot::new(device));
        }
        i
    }

    /// Composite `layers` (bottom first) over `target`, an `out`-sized
    /// working-format picture of a sequence whose format is `seq_size`.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        out: (u32, u32),
        layers: &[RenderLayer],
        seq_size: (u32, u32),
        space: WorkingSpace,
        next: &mut usize,
    ) {
        for layer in layers {
            match &layer.transition {
                None => self.draw_layer(device, queue, enc, target, out, layer, seq_size, space, next),
                Some(tr) => self.draw_transition(device, queue, enc, target, out, layer, tr, seq_size, space, next),
            }
        }
    }

    /// Both sides of a transition drawn on their own, mixed by its shader,
    /// and the mix laid over `target`.
    #[allow(clippy::too_many_arguments)]
    fn draw_transition(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        out: (u32, u32),
        layer: &RenderLayer,
        tr: &RenderTransition,
        seq_size: (u32, u32),
        space: WorkingSpace,
        next: &mut usize,
    ) {
        let i = self.slot(device, next);
        // Both sides carry mip chains, for shaders that blur or zoom.
        let key = (out.0, out.1, full_levels(out.0, out.1));
        if self.slots[i].trans.is_none() || self.slots[i].trans_key != key {
            let s = &mut self.slots[i];
            s.trans = Some([layer_tex(device, out.0, out.1, key.2), layer_tex(device, out.0, out.1, key.2)]);
            s.trans_key = key;
            s.trans_mip_bg = [Vec::new(), Vec::new()];
        }
        Self::target(&mut self.slots[i].trans_mix, device, "transition", out.0, out.1, WORKING_FORMAT, LAYER_USAGE);
        let [ta, tb] = self.slots[i].trans.as_ref().unwrap();
        let (a, b, a_all, b_all) = (ta.base.clone(), tb.base.clone(), ta.all.clone(), tb.all.clone());
        let c = self.slots[i].trans_mix.as_ref().unwrap().view.clone();
        let (a, b, c) = (&a, &b, &c);
        for v in [a, b] {
            let _clear = begin(enc, v, Some(wgpu::Color::TRANSPARENT));
        }
        let plain = |l: &RenderLayer| RenderLayer { transition: None, ..l.clone() };
        let (going, coming) = if tr.self_incoming { (None, Some(plain(layer))) } else { (Some(plain(layer)), tr.incoming.as_ref().map(plain)) };
        if let Some(l) = &going {
            self.draw_layer(device, queue, enc, a, out, l, seq_size, space, next);
        }
        if let Some(l) = &coming {
            self.draw_layer(device, queue, enc, b, out, l, seq_size, space, next);
        }
        // The mix: the transition's shader, or a cut at half-way without one.
        let mixed = if self.effect_pipeline(device, &tr.effect).is_some() {
            for k in 0..2 {
                if self.slots[i].trans_mip_bg[k].is_empty() {
                    let bgs = self.mip_groups(device, &self.slots[i].trans.as_ref().unwrap()[k]);
                    self.slots[i].trans_mip_bg[k] = bgs;
                }
                let s = &self.slots[i];
                self.mip_passes(enc, &s.trans.as_ref().unwrap()[k], &s.trans_mip_bg[k]);
            }
            let mut vals = vec![0f32; 64 * 4 + 4];
            for (n, p) in tr.effect.params.iter().take(64).enumerate() {
                vals[n * 4..n * 4 + 4].copy_from_slice(p);
            }
            vals[256] = tr.effect.time;
            vals[257] = tr.progress.clamp(0.0, 1.0);
            vals[258] = out.0 as f32 / seq_size.0.max(1) as f32;
            queue.write_buffer(&self.slots[i].trans_buf, 0, &floats(&vals));
            let bg = self.bind(device, &self.effect_layout, &self.slots[i].trans_buf, &[&a_all, &self.dummy_view, &b_all]);
            let mut pass = begin(enc, c, Some(wgpu::Color::TRANSPARENT));
            pass.set_pipeline(self.effects[&tr.effect.key].as_ref().unwrap());
            pass.set_bind_group(0, &bg, &[]);
            pass.draw(0..3, 0..1);
            c
        } else if tr.progress < 0.5 {
            a
        } else {
            b
        };
        // Laid down whole-frame, over what is beneath.
        let full: [f32; 16] = [-1.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 0.0, -1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 1.0, 1.0];
        let mut u = full.to_vec();
        u.extend([1.0, 0.0, 0.0, 0.0]);
        queue.write_buffer(&self.slots[i].ident_buf, 0, &floats(&u));
        let bg = self.bind(device, &self.one_layout, &self.slots[i].ident_buf, &[mixed]);
        let mut pass = begin(enc, target, None);
        pass.set_pipeline(&self.composite[&Blend::Normal]);
        pass.set_bind_group(0, &bg, &[]);
        pass.draw(0..6, 0..1);
    }

    /// One layer: its picture (decoded, generated or nested), its effects,
    /// a mip chain when small, then composited over `target` by Motion.
    #[allow(clippy::too_many_arguments)]
    fn draw_layer(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        enc: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        out: (u32, u32),
        layer: &RenderLayer,
        seq_size: (u32, u32),
        space: WorkingSpace,
        next: &mut usize,
    ) {
        let i = self.slot(device, next);
        let (tw, th) = match &layer.source {
            LayerSource::Frame(f) => (f.width, f.height),
            // Drawn at the output's resolution, like a CPU generator.
            LayerSource::Shader(_) => {
                let k = out.0 as f32 / seq_size.0.max(1) as f32;
                (((layer.size.0 as f32 * k).round() as u32).max(1), ((layer.size.1 as f32 * k).round() as u32).max(1))
            }
            LayerSource::Nested(n) => (n.width, n.height),
        };
        if let LayerSource::Frame(f) = &layer.source {
            if !self.upload(device, queue, i, f) {
                return;
            }
        }
        // The stored picture's extent along the upright picture's width.
        let turned = layer.turn % 180 == 90;
        let across = if turned { th } else { tw };
        // Output pixels per texture pixel decides the mip chain.
        let scale = layer.motion.scale / 100.0 * out.0 as f32 / seq_size.0.max(1) as f32 * layer.size.0 as f32 / across.max(1) as f32;
        // Effects may read a mip chain (wide blurs); otherwise only a
        // layer drawn small needs one.
        let levels = if layer.effects.is_empty() { mip_levels(scale, tw, th) } else { full_levels(tw, th) };
        self.layer_pair(device, i, tw, th, levels);
        // Texture pixels per pixel of the picture at full quality: what an
        // effect multiplies its sizes by (a proxy is smaller; a clip at any
        // preview quality is decoded whole).
        let fx_scale = across as f32 / layer.size.0.max(1) as f32;
        let first = self.slots[i].pair.as_ref().unwrap()[0].base.clone();

        // 1. The picture, into the first texture of the pair.
        match &layer.source {
            LayerSource::Frame(f) => {
                let m = columns(ycbcr(&f.color.matrix, f.height));
                let (deep, rgba) = self.slots[i].planes.as_ref().map_or((false, false), |p| (p.key.2, p.rgba));
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
                    let bg = if rgba {
                        self.bind(device, &self.one_layout, &s.decode_buf, &[&p.luma_view])
                    } else {
                        self.bind(device, &self.decode_layout, &s.decode_buf, &[&p.luma_view, &p.chroma_view])
                    };
                    self.slots[i].decode_bg = Some(bg);
                }
                let s = &self.slots[i];
                let mut pass = begin(enc, &first, Some(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(if rgba { &self.decode_rgba } else { &self.decode });
                pass.set_bind_group(0, s.decode_bg.as_ref(), &[]);
                pass.draw(0..3, 0..1);
            }
            LayerSource::Shader(e) => {
                if self.effect_pipeline(device, e).is_none() {
                    return;
                }
                let mut vals = vec![0f32; 64 * 4 + 4];
                for (n, p) in e.params.iter().take(64).enumerate() {
                    vals[n * 4..n * 4 + 4].copy_from_slice(p);
                }
                vals[256] = e.time;
                vals[258] = fx_scale;
                queue.write_buffer(&self.slots[i].trans_buf, 0, &floats(&vals));
                let bg = self.bind(device, &self.effect_layout, &self.slots[i].trans_buf, &[&self.dummy_view, &self.dummy_view, &self.dummy_view]);
                let mut pass = begin(enc, &first, Some(wgpu::Color::TRANSPARENT));
                pass.set_pipeline(self.effects[&e.key].as_ref().unwrap());
                pass.set_bind_group(0, &bg, &[]);
                pass.draw(0..3, 0..1);
            }
            LayerSource::Nested(n) => {
                {
                    let _clear = begin(enc, &first, Some(wgpu::Color::TRANSPARENT));
                }
                self.draw(device, queue, enc, &first, (n.width, n.height), &n.layers, n.seq_size, space, next);
            }
        }
        let mut cur = 0;

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
            vals[258] = fx_scale;
            self.layer_mips(device, enc, i, cur);
            let s = &mut self.slots[i];
            while s.effect_bufs.len() <= k {
                s.effect_bufs.push(uniform_buf(device, EFFECT_BYTES));
                s.effect_bg.push([None, None]);
            }
            queue.write_buffer(&s.effect_bufs[k], 0, &floats(&vals));
            if self.slots[i].effect_bg[k][cur].is_none() {
                let s = &self.slots[i];
                // Bindings: source (with its mip chain), (unused slot), source_b.
                let src = &s.pair.as_ref().unwrap()[cur].all;
                let bg = self.bind(device, &self.effect_layout, &s.effect_bufs[k], &[src, &self.dummy_view, &self.dummy_view]);
                self.slots[i].effect_bg[k][cur] = Some(bg);
            }
            let s = &self.slots[i];
            let nxt = cur ^ 1;
            let pipe = self.effects[&e.key].as_ref().unwrap();
            let mut pass = begin(enc, &s.pair.as_ref().unwrap()[nxt].base, Some(wgpu::Color::TRANSPARENT));
            pass.set_pipeline(pipe);
            pass.set_bind_group(0, s.effect_bg[k][cur].as_ref(), &[]);
            pass.draw(0..3, 0..1);
            drop(pass);
            cur = nxt;
        }

        // 3. Mip chain, when the layer is drawn below half size (or has
        // effects): each level a 2×2 average of the one above.
        if levels > 1 {
            self.layer_mips(device, enc, i, cur);
        }

        // 4. Composite, by Motion, in the picture's logical size.
        let mut corners = quad(&layer.motion, (layer.size.0 as f32, layer.size.1 as f32), (seq_size.0 as f32, seq_size.1 as f32));
        // Upright: each corner shows the stored picture's point under it.
        for c in &mut corners {
            [c[2], c[3]] = turn_uv([c[2], c[3]], layer.turn);
        }
        let mut u: Vec<f32> = corners.iter().flatten().copied().collect();
        u.extend([layer.opacity.clamp(0.0, 1.0), 0.0, 0.0, 0.0]);
        queue.write_buffer(&self.slots[i].quad_buf, 0, &floats(&u));
        if self.slots[i].quad_bg[cur].is_none() {
            let s = &self.slots[i];
            let bg = self.bind(device, &self.one_layout, &s.quad_buf, &[&s.pair.as_ref().unwrap()[cur].all]);
            self.slots[i].quad_bg[cur] = Some(bg);
        }
        let s = &self.slots[i];
        let mut pass = begin(enc, target, None);
        pass.set_pipeline(&self.composite[&layer.blend]);
        pass.set_bind_group(0, s.quad_bg[cur].as_ref(), &[]);
        pass.draw(0..6, 0..1);
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
        let bpp = if out.tex.format() == DEEP_FORMAT { 8 } else { 4 };
        let row = (w * bpp).div_ceil(256) * 256;
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
        Some(Readback { buf, w, h, bpp, row, index, rx, pool: self.readback_pool.clone() })
    }
}

/// A monitor image on its way back from the GPU.
pub struct Readback {
    buf: wgpu::Buffer,
    w: u32,
    h: u32,
    /// 4 (RGBA8) or 8 (16-bit RGBA, little-endian).
    bpp: u32,
    row: u32,
    index: wgpu::SubmissionIndex,
    rx: std::sync::mpsc::Receiver<Result<(), wgpu::BufferAsyncError>>,
    pool: Arc<std::sync::Mutex<Vec<wgpu::Buffer>>>,
}

impl Readback {
    /// 4 for RGBA8, 8 for the deep output's 16-bit RGBA (little-endian).
    pub fn bytes_per_pixel(&self) -> u32 {
        self.bpp
    }

    /// Wait for this copy (only this one, not work queued after it) and
    /// return tightly packed rows (see [`Readback::bytes_per_pixel`]).
    pub fn finish(self, device: &wgpu::Device) -> Option<(u32, u32, Vec<u8>)> {
        let _ = device.poll(wgpu::PollType::Wait { submission_index: Some(self.index), timeout: None });
        self.rx.recv().ok()?.ok()?;
        let (w, h, row, bpp) = (self.w as usize, self.h as usize, self.row as usize, self.bpp as usize);
        let data = self.buf.slice(..).get_mapped_range().ok()?;
        let mut px = Vec::with_capacity(w * h * bpp);
        for y in 0..h {
            px.extend_from_slice(&data[y * row..y * row + w * bpp]);
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
/// Where an upright picture's point `uv` lies in the stored picture that
/// turns `turn` degrees clockwise to stand upright.
pub fn turn_uv([u, v]: [f32; 2], turn: u16) -> [f32; 2] {
    match turn % 360 {
        90 => [v, 1.0 - u],
        180 => [1.0 - u, 1.0 - v],
        270 => [1.0 - v, u],
        _ => [u, v],
    }
}

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
