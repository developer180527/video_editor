//! A frame of the sequence, resolved for the compositor: which decoded
//! pictures, transformed how, with which GPU effects and parameter values.

use std::sync::Arc;
use std::time::Duration;

use ve_media::{Lookup, VideoPool};
use ve_model::*;
use ve_plugin_host::{Implementation, ParamInfo, Registry};
use ve_render::{FramePlan, GpuEffect, RenderLayer, WorkingSpace};

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

/// Resolve `plan`. With `wait`, block for exact frames (export); without,
/// take what the decoders have (playback, scrubbing).
pub fn resolve(project: &Project, plan: FramePlan, registry: &Registry, pool: &Arc<VideoPool>, wait: Option<Duration>) -> Frame {
    let seq = project.active();
    let seq_size = seq.map(|s| (s.format.width, s.format.height)).unwrap_or((1920, 1080));
    let space = WorkingSpace::from_name(seq.map(|s| s.format.working_space.as_str()).unwrap_or("ACEScg"));
    let mut complete = true;
    let mut missing = Vec::new();
    let mut layers = Vec::new();
    for l in &plan.layers {
        let ClipSource::Asset { asset } = &l.source else { continue };
        let Some(asset) = project.assets.get(asset) else { continue };
        let frame = match wait {
            Some(timeout) => match pool.frame_blocking(asset.id, &asset.media, l.source_time, timeout) {
                Ok(f) => Some(f),
                Err(e) => {
                    missing.push(format!("{}: {e}", asset.name));
                    None
                }
            },
            None => match pool.frame(asset.id, &asset.media, l.source_time) {
                Lookup::Exact(f) => Some(f),
                Lookup::Nearest(f) => {
                    complete = false;
                    Some(f)
                }
                Lookup::Pending => {
                    complete = false;
                    None
                }
                Lookup::Failed(e) => {
                    missing.push(format!("{}: {e}", asset.name));
                    None
                }
            },
        };
        let Some(frame) = frame else { continue };
        let motion = l.motion(seq_size, (frame.width, frame.height));
        let (opacity, blend) = l.opacity();
        let effects = l
            .effects
            .iter()
            .filter_map(|e| {
                let info = registry.find(&e.plugin)?;
                if matches!(info.implementation, Implementation::Intrinsic) {
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
        layers.push(RenderLayer { frame, motion, opacity, blend, effects });
    }
    Frame { plan, layers, seq_size, space, complete, missing }
}

/// Ask the decoders to get ready for the clips that start on each video track
/// within `ahead` of `t`, so a cut plays without a stall.
pub fn prefetch(project: &Project, t: ve_time::Time, ahead: ve_time::Time, pool: &Arc<VideoPool>) {
    let Some(seq) = project.active() else { return };
    for track in seq.tracks.iter().filter(|tr| tr.kind == TrackKind::Video && tr.enabled) {
        let next = track.clips.get(track.insertion_index(t)).filter(|c| c.enabled && c.timeline_start <= t + ahead);
        if let Some(ClipSource::Asset { asset }) = next.map(|c| &c.source) {
            if let Some(a) = project.assets.get(asset) {
                pool.prefetch(a.id, &a.media, next.unwrap().source_range.start);
            }
        }
    }
}
