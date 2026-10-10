//! Video scopes: waveform, RGB parade, vectorscope and histogram of the
//! program monitor's picture, measured on the GPU.
//!
//! Two compute passes. The first reads the display-encoded picture (what
//! the monitor shows, so levels read as 0–100 IRE / 0–255) and counts
//! pixels into bins with atomics; the second draws the bins into an RGBA
//! image the UI shows. The graticule (scales, targets) is the UI's.

use wgpu::util::DeviceExt;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScopeKind {
    /// Luma per column, 0 at the bottom, 255 at the top.
    #[default]
    Waveform,
    /// The waveform of red, green and blue side by side.
    Parade,
    /// Chroma: Cb right, Cr up, saturation as distance from the centre.
    Vectorscope,
    /// How many pixels at each level, per channel.
    Histogram,
}

impl ScopeKind {
    pub const ALL: [ScopeKind; 4] = [ScopeKind::Waveform, ScopeKind::Parade, ScopeKind::Vectorscope, ScopeKind::Histogram];

    pub fn name(self) -> &'static str {
        match self {
            ScopeKind::Waveform => "Waveform (Luma)",
            ScopeKind::Parade => "Parade (RGB)",
            ScopeKind::Vectorscope => "Vectorscope YUV",
            ScopeKind::Histogram => "Histogram",
        }
    }

    /// The image's size.
    pub fn size(self) -> (u32, u32) {
        match self {
            ScopeKind::Vectorscope => (256, 256),
            _ => (COLUMNS, 256),
        }
    }

    fn index(self) -> u32 {
        self as u32
    }
}

/// Waveform columns (a parade gives each channel a third).
const COLUMNS: u32 = 768;
/// The picture is sampled on a grid at most this wide.
const SAMPLE_W: u32 = 960;

const SHADER: &str = r#"
struct P {
    kind: u32,
    src_w: u32,
    src_h: u32,
    step: u32,
    out_w: u32,
    out_h: u32,
    rows: u32,
    _pad: u32,
};
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> bins: array<atomic<u32>>;
@group(0) @binding(2) var<uniform> p: P;
@group(0) @binding(3) var out: texture_storage_2d<rgba8unorm, write>;

fn level(v: f32) -> u32 {
    return u32(clamp(v, 0.0, 1.0) * 255.0 + 0.5);
}
fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

@compute @workgroup_size(16, 16)
fn accumulate(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x * p.step;
    let y = id.y * p.step;
    if (x >= p.src_w || y >= p.src_h) { return; }
    let c = clamp(textureLoad(src, vec2<i32>(i32(x), i32(y)), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
    switch p.kind {
        // A sample covers every column its span of the picture maps to, so
        // a picture narrower than the scope leaves no gaps.
        case 0u: {
            let c0 = x * p.out_w / p.src_w;
            let c1 = max(c0 + 1u, min(x + p.step, p.src_w) * p.out_w / p.src_w);
            for (var col = c0; col < c1; col++) {
                atomicAdd(&bins[col * 256u + level(luma(c))], 1u);
            }
        }
        case 1u: {
            let third = p.out_w / 3u;
            let c0 = x * third / p.src_w;
            let c1 = max(c0 + 1u, min(x + p.step, p.src_w) * third / p.src_w);
            for (var ch = 0u; ch < 3u; ch++) {
                for (var col = c0; col < c1; col++) {
                    atomicAdd(&bins[(ch * third + col) * 256u + level(c[ch])], 1u);
                }
            }
        }
        case 2u: {
            let yl = luma(c);
            let cb = (c.b - yl) / 1.8556;
            let cr = (c.r - yl) / 1.5748;
            // Full scale is a 75% bar's chroma at the graticule's edge.
            let u = level(cb + 0.5);
            let v = level(0.5 - cr);
            atomicAdd(&bins[v * 256u + u], 1u);
        }
        default: {
            for (var ch = 0u; ch < 3u; ch++) {
                atomicAdd(&bins[ch * 256u + level(c[ch])], 1u);
            }
        }
    }
}

// How bright a bin of `n` samples draws: soft, so a gradient shows and a
// flat field glows rather than clipping to a block.
fn glow(n: u32, per_bin: f32) -> f32 {
    return 1.0 - exp(-f32(n) / max(per_bin, 1.0) * 1.5);
}

@compute @workgroup_size(16, 16)
fn draw(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= p.out_w || id.y >= p.out_h) { return; }
    var rgb = vec3<f32>(0.0);
    let row = 255u - id.y * 256u / p.out_h;
    switch p.kind {
        case 0u: {
            let n = atomicLoad(&bins[id.x * 256u + row]);
            rgb = vec3<f32>(0.78, 0.94, 0.80) * glow(n, f32(p.rows) / 48.0);
        }
        case 1u: {
            let third = p.out_w / 3u;
            let ch = min(id.x / third, 2u);
            let n = atomicLoad(&bins[id.x * 256u + row]);
            var tint = vec3<f32>(0.0);
            tint[ch] = 1.0;
            rgb = (tint * 0.85 + vec3<f32>(0.15)) * glow(n, f32(p.rows) / 48.0);
        }
        case 2u: {
            let n = atomicLoad(&bins[id.y * 256u + id.x]);
            rgb = vec3<f32>(0.80, 0.95, 0.85) * glow(n, f32(p.rows * p.out_w) / 20000.0);
        }
        default: {
            let bin = id.x * 256u / p.out_w;
            // Heights on a log scale, against a quarter of the samples in
            // one bin: a flat field reaches the top, detail stays visible.
            let full = log(1.0 + f32(p.rows * p.src_w / (p.step * p.step)) / 4.0);
            let h = f32(p.out_h - 1u - id.y) / f32(p.out_h);
            for (var ch = 0u; ch < 3u; ch++) {
                let n = atomicLoad(&bins[ch * 256u + bin]);
                if (log(1.0 + f32(n)) / full > h) {
                    var tint = vec3<f32>(0.0);
                    tint[ch] = 0.7;
                    rgb = rgb + tint;
                }
            }
        }
    }
    textureStore(out, vec2<i32>(i32(id.x), i32(id.y)), vec4<f32>(rgb, 1.0));
}
"#;

pub struct Scopes {
    layout: wgpu::BindGroupLayout,
    accumulate: wgpu::ComputePipeline,
    draw: wgpu::ComputePipeline,
    bins: wgpu::Buffer,
    output: Option<(ScopeKind, wgpu::Texture)>,
}

impl Scopes {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("scopes"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let entry = |binding, ty| wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::COMPUTE, ty, count: None };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("scopes"),
            entries: &[
                entry(0, wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: false }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }),
                entry(1, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }),
                entry(2, wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }),
                entry(3, wgpu::BindingType::StorageTexture { access: wgpu::StorageTextureAccess::WriteOnly, format: wgpu::TextureFormat::Rgba8Unorm, view_dimension: wgpu::TextureViewDimension::D2 }),
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("scopes"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let bins = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("scope bins"),
            size: (COLUMNS as u64) * 256 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        Scopes { accumulate: pipeline("accumulate"), draw: pipeline("draw"), layout, bins, output: None }
    }

    /// Measure `picture` (the monitor's display-encoded output) as `kind`;
    /// the image to show is [`Scopes::output`].
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, picture: &wgpu::Texture, kind: ScopeKind) {
        let (out_w, out_h) = kind.size();
        if self.output.as_ref().is_none_or(|(k, _)| *k != kind) {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("scope"),
                size: wgpu::Extent3d { width: out_w, height: out_h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            self.output = Some((kind, tex));
        }
        let out = &self.output.as_ref().unwrap().1;
        let (src_w, src_h) = (picture.width(), picture.height());
        let step = src_w.div_ceil(SAMPLE_W).max(1);
        // The shader's `P`, field for field.
        let params = [kind.index(), src_w, src_h, step, out_w, out_h, src_h.div_ceil(step), 0];
        let bytes: Vec<u8> = params.iter().flat_map(|v| v.to_le_bytes()).collect();
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("scope params"), contents: &bytes, usage: wgpu::BufferUsages::UNIFORM });
        let src_view = picture.create_view(&Default::default());
        let out_view = out.create_view(&Default::default());
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("scopes"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&src_view) },
                wgpu::BindGroupEntry { binding: 1, resource: self.bins.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&out_view) },
            ],
        });
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("scopes") });
        enc.clear_buffer(&self.bins, 0, None);
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("scope accumulate"), timestamp_writes: None });
            pass.set_pipeline(&self.accumulate);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(src_w.div_ceil(step).div_ceil(16), src_h.div_ceil(step).div_ceil(16), 1);
        }
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("scope draw"), timestamp_writes: None });
            pass.set_pipeline(&self.draw);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(out_w.div_ceil(16), out_h.div_ceil(16), 1);
        }
        queue.submit([enc.finish()]);
    }

    /// The last scope drawn.
    pub fn output(&self) -> Option<&wgpu::Texture> {
        self.output.as_ref().map(|(_, t)| t)
    }
}
