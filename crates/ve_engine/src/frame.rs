//! A frame of the sequence, resolved for the compositor: which pictures —
//! decoded (from originals or proxies), generated, or nested sequences —
//! transformed how, with which GPU effects, through which transitions.

use std::sync::Arc;
use std::time::Duration;

use ve_media::{Lookup, VideoPool};
use ve_model::*;
use ve_plugin_host::{intrinsic, EffectKind, Implementation, ParamInfo, Registry};
use ve_render::{FramePlan, GpuEffect, Layer, LayerSource, NestedLayers, Quality, RenderLayer, RenderTransition, WorkingSpace};

pub struct Frame {
    pub plan: FramePlan,
    pub layers: Vec<RenderLayer>,
    pub seq_size: (u32, u32),
    pub space: WorkingSpace,
    /// Every layer shows its exact frame (false while decoding catches up).
    pub complete: bool,
    /// Layers that could not be shown at all, and why ("clip.mov: not found").
    pub missing: Vec<String>,
}

fn as_vec4(v: &Value) -> [f32; 4] {
    match v {
        Value::Bool(b) => [*b as u8 as f32, 0.0, 0.0, 0.0],
        Value::Int(i) => [*i as f32, 0.0, 0.0, 0.0],
        Value::Float(f) => [*f as f32, 0.0, 0.0, 0.0],
        Value::Vec2([x, y]) => [*x as f32, *y as f32, 0.0, 0.0],
        Value::Color(c) => *c,
        Value::Choice(c) => [*c as f32, 0.0, 0.0, 0.0],
        Value::Text(_) => [0.0; 4],
    }
}

fn param_values(info: &[ParamInfo], given: &[(String, Value)]) -> Vec<[f32; 4]> {
    info.iter()
        .map(|p| given.iter().find(|(k, _)| *k == p.id).map(|(_, v)| as_vec4(v)).unwrap_or(p.default.map(|d| d as f32)))
        .collect()
}

/// Resolve `plan` (of the active sequence). With `wait`, block for exact
/// frames (export); without, take what the decoders have (playback).
pub fn resolve(project: &Project, plan: FramePlan, registry: &Registry, pool: &Arc<VideoPool>, wait: Option<Duration>) -> Frame {
    let seq = project.active();
    let seq_size = seq.map(|s| (s.format.width, s.format.height)).unwrap_or((1920, 1080));
    let space = WorkingSpace::from_name(seq.map(|s| s.format.working_space.as_str()).unwrap_or("ACEScg"));
    let scale = plan.width as f32 / seq_size.0.max(1) as f32;
    let mut r = Resolver { project, registry, pool, wait, proxies: plan.proxies, scale, complete: true, missing: Vec::new() };
    let layers = plan.layers.iter().filter_map(|l| r.layer(l, seq_size)).collect();
    let (complete, missing) = (r.complete, r.missing);
    Frame { plan, layers, seq_size, space, complete, missing }
}

struct Resolver<'a> {
    project: &'a Project,
    registry: &'a Registry,
    pool: &'a Arc<VideoPool>,
    wait: Option<Duration>,
    proxies: bool,
    /// Output pixels per sequence pixel (preview quality).
    scale: f32,
    complete: bool,
    missing: Vec<String>,
}

impl Resolver<'_> {
    /// A decoded picture of `media` at `t`, as the mode allows.
    fn picture(&mut self, name: &str, media: &MediaRef, t: ve_time::Time) -> Option<Arc<ve_ports::VideoFrame>> {
        match self.wait {
            Some(timeout) => match self.pool.frame_blocking(media, t, timeout) {
                Ok(f) => Some(f),
                Err(e) => {
                    self.missing.push(format!("{name}: {e}"));
                    None
                }
            },
            None => match self.pool.frame(media, t) {
                Lookup::Exact(f) => Some(f),
                Lookup::Nearest(f) => {
                    self.complete = false;
                    Some(f)
                }
                Lookup::Pending => {
                    self.complete = false;
                    None
                }
                Lookup::Failed(e) => {
                    self.missing.push(format!("{name}: {e}"));
                    None
                }
            },
        }
    }

    /// One evaluated layer of a sequence whose format is `seq_size`.
    fn layer(&mut self, l: &Layer, seq_size: (u32, u32)) -> Option<RenderLayer> {
        let out = |n: u32| ((n as f32 * self.scale).round() as u32).max(1);
        // The picture, and its size as Motion sees it.
        let (source, size, generator) = match &l.source {
            ClipSource::Asset { asset, .. } => {
                let a = self.project.assets.get(asset)?;
                let frame = self.picture(&a.name, a.picture_media(self.proxies), l.source_time)?;
                // A proxy stands in at its original's size.
                let size = a.info.as_ref().and_then(|i| i.video.as_ref()).map_or((frame.width, frame.height), |v| (v.width, v.height));
                (LayerSource::Frame(frame), size, None)
            }
            ClipSource::Generator { plugin } => {
                // Its parameters are its own effect on the clip.
                let params = l.effects.iter().find(|e| e.plugin == *plugin).map(|e| e.params.clone()).unwrap_or_default();
                let source = match ve_render::generate::picture(&plugin.id, &params, out(seq_size.0), out(seq_size.1)) {
                    Some(f) => LayerSource::Frame(f),
                    None => {
                        let info = self.registry.find(plugin).filter(|i| i.kind == EffectKind::Generator)?;
                        LayerSource::Shader(GpuEffect {
                            key: format!("{}@{}", plugin.id, plugin.major_version),
                            wgsl: info.wgsl.as_deref()?.into(),
                            params: param_values(&info.params, &params),
                            time: l.clip_time.as_seconds_f64() as f32,
                        })
                    }
                };
                (source, seq_size, Some(plugin.clone()))
            }
            ClipSource::Sequence { sequence, angle } => {
                let nested = self.project.sequences.get(sequence)?.clone();
                let quality = Quality { scale: self.scale, use_proxies: self.proxies };
                let mut plan = ve_render::evaluate(&nested, l.source_time, quality);
                // Multicam: just the chosen angle (a video track of the nest).
                if let Some(a) = angle {
                    let track = nested.tracks.iter().filter(|t| t.kind == TrackKind::Video).nth(*a as usize).map(|t| t.id);
                    plan.layers.retain(|x| Some(x.track) == track);
                }
                let nsize = (nested.format.width, nested.format.height);
                let layers = plan.layers.iter().filter_map(|x| self.layer(x, nsize)).collect();
                (LayerSource::Nested(Box::new(NestedLayers { width: plan.width, height: plan.height, seq_size: nsize, layers })), nsize, None)
            }
        };
        let motion = l.motion(seq_size, size);
        let (opacity, blend) = l.opacity();
        let effects = l
            .effects
            .iter()
            .filter(|e| Some(&e.plugin) != generator.as_ref()) // a generator's own parameters are not a filter
            .filter_map(|e| {
                let info = self.registry.find(&e.plugin)?;
                if matches!(info.implementation, Implementation::Intrinsic) || info.kind != EffectKind::Filter {
                    return None;
                }
                Some(GpuEffect {
                    key: format!("{}@{}", e.plugin.id, e.plugin.major_version),
                    wgsl: info.wgsl.as_deref()?.into(),
                    params: param_values(&info.params, &e.params),
                    time: l.clip_time.as_seconds_f64() as f32,
                })
            })
            .collect();
        let transition = l.transition.as_ref().map(|t| {
            // A transition without a picture of its own (an audio crossfade
            // put on video) dissolves.
            let info = self.registry.find(&t.plugin).filter(|i| i.wgsl.is_some()).or_else(|| self.registry.find(&intrinsic::plugin_ref(intrinsic::DISSOLVE)));
            let effect = info.map_or_else(
                || GpuEffect { key: String::new(), wgsl: "".into(), params: vec![], time: 0.0 },
                |i| GpuEffect {
                    key: format!("{}@{}", i.plugin.id, i.plugin.major_version),
                    wgsl: i.wgsl.as_deref().unwrap_or_default().into(),
                    params: param_values(&i.params, &t.params),
                    time: 0.0,
                },
            );
            let incoming = t.incoming.as_ref().and_then(|x| self.layer(x, seq_size));
            Box::new(RenderTransition { effect, progress: t.progress as f32, incoming, self_incoming: t.self_incoming })
        });
        Some(RenderLayer { source, size, motion, opacity, blend, effects, transition })
    }
}

/// Ask the decoders to get ready for the clips that start on each video track
/// within `ahead` of `t`, so a cut plays without a stall.
pub fn prefetch(project: &Project, t: ve_time::Time, ahead: ve_time::Time, proxies: bool, pool: &Arc<VideoPool>) {
    let Some(seq) = project.active() else { return };
    for track in seq.tracks.iter().filter(|tr| tr.kind == TrackKind::Video && tr.enabled) {
        let next = track.clips.get(track.insertion_index(t)).filter(|c| c.enabled && c.timeline_start <= t + ahead);
        if let Some((ClipSource::Asset { asset, .. }, c)) = next.map(|c| (&c.source, c)) {
            if let Some(a) = project.assets.get(asset) {
                pool.prefetch(a.picture_media(proxies), c.media_time(ve_time::Time::ZERO));
            }
        }
    }
}
