//! A window's picture kept between frames, so a frame repaints only its
//! damage (see `damage`) and copies the result to the swapchain image, whose
//! contents are not kept.

use crate::damage::{Damage, Tracker};
use libgui::{Backend, Batch};

pub struct Canvas {
    format: wgpu::TextureFormat,
    tex: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// Fills a damaged rectangle with the clear colour (a render pass can
    /// only clear all of its target). Remade when the colour changes.
    fill: Option<([f32; 4], wgpu::RenderPipeline)>,
}

impl Canvas {
    pub fn new(format: wgpu::TextureFormat) -> Self {
        Canvas {
            format,
            tex: None,
            fill: None,
        }
    }

    /// Repaint `damage` of the canvas (made `size` if it is not; then all of
    /// it) and copy the canvas onto `target`.
    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        device: &wgpu::Device,
        enc: &mut wgpu::CommandEncoder,
        renderer: &mut libgui_wgpu::Renderer,
        batches: &[Batch],
        damage: &Damage,
        tracker: &Tracker,
        clear: [f32; 4],
        size: (u32, u32),
        target: &wgpu::Texture,
    ) {
        let fresh = self
            .tex
            .as_ref()
            .is_none_or(|(t, _)| (t.width(), t.height()) != size);
        if fresh {
            let tex = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("ui canvas"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let view = tex.create_view(&Default::default());
            self.tex = Some((tex, view));
        }
        let rects = match damage {
            Damage::Rects(r) if !fresh => Some(r.as_slice()),
            _ => None,
        };
        if rects.is_some() && self.fill.as_ref().is_none_or(|(c, _)| *c != clear) {
            self.fill = Some((clear, fill_pipeline(device, self.format, clear)));
        }
        let (tex, view) = self.tex.as_ref().unwrap();
        {
            let [r, g, b, a] = clear.map(f64::from);
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("ui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: if rects.is_some() {
                            wgpu::LoadOp::Load
                        } else {
                            wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a })
                        },
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            match rects {
                None => renderer.render_batches(&mut pass, batches),
                Some(rects) => {
                    let fill = &self.fill.as_ref().unwrap().1;
                    let mut some = Vec::new();
                    for r in rects {
                        tracker.within(batches, *r, &mut some);
                        pass.set_scissor_rect(r[0], r[1], r[2] - r[0], r[3] - r[1]);
                        pass.set_pipeline(fill);
                        pass.draw(0..3, 0..1);
                        renderer.render_batches(&mut pass, &some);
                    }
                }
            }
        }
        enc.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
        );
    }
}

/// A triangle over the whole target in one colour, written as is (the clear
/// colour a render pass would write).
fn fill_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    c: [f32; 4],
) -> wgpu::RenderPipeline {
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("ui fill"),
        source: wgpu::ShaderSource::Wgsl(
            "override r: f32; override g: f32; override b: f32; override a: f32;
             @vertex fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
                 let p = vec2(f32((i << 1u) & 2u), f32(i & 2u));
                 return vec4(p * 2.0 - 1.0, 0.0, 1.0);
             }
             @fragment fn fs() -> @location(0) vec4<f32> { return vec4(r, g, b, a); }"
                .into(),
        ),
    });
    let constants = [
        ("r", c[0] as f64),
        ("g", c[1] as f64),
        ("b", c[2] as f64),
        ("a", c[3] as f64),
    ];
    let options = wgpu::PipelineCompilationOptions {
        constants: &constants,
        ..Default::default()
    };
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("ui fill"),
        layout: None,
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs"),
            compilation_options: options.clone(),
            buffers: &[],
        },
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs"),
            compilation_options: options,
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: None,
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        multiview_mask: None,
        cache: None,
    })
}
