//! Rendering is a pure function of (snapshot, time, quality).
//!
//! Two halves:
//! - [`evaluate`] decides *what* is visible at a time: which clips, which
//!   source frames, which effects with which parameter values. No GPU, no
//!   I/O, fully testable. Playback, scrubbing, thumbnails and export all start
//!   here, so they cannot disagree.
//! - [`Compositor`] draws an evaluated frame with wgpu into an RGBA16F,
//!   linear-light target. wgpu is the GPU layer on every platform (Metal on
//!   Apple, Vulkan/D3D12 elsewhere), so this is not a port.

mod compositor;
pub mod generate;

pub use compositor::{quad, yuv_levels, Blend, Compositor, LayerSource, NestedLayers, Readback, RenderTransition, DEEP_FORMAT, GpuEffect, Motion, RenderLayer, TextureImporter, WorkingSpace, DISPLAY_FORMAT, WORKING_FORMAT};

use std::sync::Arc;
use ve_model::*;
use ve_time::{Time, TimeRange};

/// How hard to try. Previews drop resolution; export never does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quality {
    /// 1.0 full, 0.5 half, 0.25 quarter.
    pub scale: f32,
    pub use_proxies: bool,
}

impl Quality {
    pub const FULL: Quality = Quality { scale: 1.0, use_proxies: false };
    pub const PREVIEW: Quality = Quality { scale: 0.5, use_proxies: true };
}

/// One effect with its parameter values resolved at this time.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedEffect {
    pub plugin: PluginRef,
    pub params: Vec<(String, Value)>,
}

/// One visible picture, bottom to top.
#[derive(Clone, Debug, PartialEq)]
pub struct Layer {
    pub track: TrackId,
    pub clip: ClipId,
    pub source: ClipSource,
    /// The source frame to show (through the clip's speed or remap).
    pub source_time: Time,
    /// Time since the clip's start: what keyframes and effects see. Outside
    /// `0..duration` inside a transition (the clip's handles).
    pub clip_time: Time,
    pub effects: Vec<ResolvedEffect>,
    /// Inside a transition: the other clip and how far along it is.
    pub transition: Option<Box<LayerTransition>>,
}

/// A layer's part in a transition. The layer is the clip going out, unless
/// it is fading in from nothing (`self_incoming`).
#[derive(Clone, Debug, PartialEq)]
pub struct LayerTransition {
    pub plugin: PluginRef,
    pub params: Vec<(String, Value)>,
    /// 0 at the transition's start, 1 at its end.
    pub progress: f64,
    /// The clip coming in, for a cross transition; `None` for a fade.
    pub incoming: Option<Layer>,
    /// This layer is the one coming in (a fade from nothing).
    pub self_incoming: bool,
}

/// Everything needed to draw one frame of a sequence.
#[derive(Clone, Debug, PartialEq)]
pub struct FramePlan {
    pub time: Time,
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
    /// Read pictures from proxies where assets have them.
    pub proxies: bool,
}

impl ResolvedEffect {
    fn float(&self, name: &str) -> Option<f32> {
        self.params.iter().find(|(k, _)| k == name).and_then(|(_, v)| match v {
            Value::Float(f) => Some(*f as f32),
            Value::Int(i) => Some(*i as f32),
            _ => None,
        })
    }

    fn vec2(&self, name: &str) -> Option<[f32; 2]> {
        self.params.iter().find(|(k, _)| k == name).and_then(|(_, v)| match v {
            Value::Vec2([x, y]) => Some([*x as f32, *y as f32]),
            _ => None,
        })
    }
}

impl Layer {
    fn intrinsic(&self, id: &str) -> Option<&ResolvedEffect> {
        self.effects.iter().find(|e| e.plugin.id == id)
    }

    /// Motion (`ve.motion`), or a centred, unscaled image when it is absent.
    pub fn motion(&self, seq: (u32, u32), src: (u32, u32)) -> Motion {
        let centre = |w: u32, h: u32| [w as f32 / 2.0, h as f32 / 2.0];
        let m = self.intrinsic("ve.motion");
        let get = |n: &str, d: f32| m.and_then(|m| m.float(n)).unwrap_or(d);
        Motion {
            position: m.and_then(|m| m.vec2("position")).unwrap_or(centre(seq.0, seq.1)),
            scale: get("scale", 100.0),
            rotation: get("rotation", 0.0),
            anchor: m.and_then(|m| m.vec2("anchor")).unwrap_or(centre(src.0, src.1)),
            crop: [get("crop_left", 0.0), get("crop_top", 0.0), get("crop_right", 0.0), get("crop_bottom", 0.0)],
        }
    }

    /// Opacity (`ve.opacity`) as 0..1, and its blend mode.
    pub fn opacity(&self) -> (f32, Blend) {
        let o = self.intrinsic("ve.opacity");
        let opacity = o.and_then(|o| o.float("opacity")).unwrap_or(100.0) / 100.0;
        let blend = match o.and_then(|o| o.params.iter().find(|(k, _)| k == "blend")).map(|(_, v)| v) {
            Some(Value::Choice(1)) => Blend::Multiply,
            Some(Value::Choice(2)) => Blend::Screen,
            Some(Value::Choice(3)) => Blend::Add,
            Some(Value::Choice(4)) => Blend::Overlay,
            _ => Blend::Normal,
        };
        (opacity, blend)
    }
}

/// What the sequence shows at `t`.
pub fn evaluate(seq: &Sequence, t: Time, quality: Quality) -> FramePlan {
    let layers = seq.tracks.iter().filter(|tr| tr.kind == TrackKind::Video && tr.enabled).filter_map(|tr| track_layer(tr, t)).collect();
    FramePlan {
        time: t,
        width: ((seq.format.width as f32 * quality.scale).round() as u32).max(1),
        height: ((seq.format.height as f32 * quality.scale).round() as u32).max(1),
        layers,
        proxies: quality.use_proxies,
    }
}

/// What one track shows at `t`: a clip, or a transition between two (or
/// from or to nothing). Disabled clips show nothing.
fn track_layer(track: &Track, t: Time) -> Option<Layer> {
    let active = track.active_at(t);
    let shown = |c: &&Arc<Clip>| c.enabled;
    match active.as_slice() {
        [] => None,
        [a] if a.transition.is_none() => Some(a.clip).filter(shown).map(|c| layer(track, c, t)),
        _ => {
            let (tr, progress, _) = active.iter().find_map(|a| a.transition)?;
            let out = active.iter().find(|a| matches!(a.transition, Some((_, _, Role::Outgoing)))).map(|a| a.clip).filter(shown);
            let inc = active.iter().find(|a| matches!(a.transition, Some((_, _, Role::Incoming)))).map(|a| a.clip).filter(shown);
            let into = TimeRange::new(Time::ZERO, tr.duration());
            let local = Time::from_seconds_f64(progress * into.duration.as_seconds_f64());
            let params = tr.params.iter().map(|(k, p)| (k.clone(), p.value_at(local))).collect();
            let (mut base, incoming, self_incoming) = match (out, inc) {
                (Some(o), i) => (layer(track, o, t), i.map(|i| layer(track, i, t)), false),
                (None, Some(i)) => (layer(track, i, t), None, true),
                (None, None) => return None,
            };
            base.transition = Some(Box::new(LayerTransition { plugin: tr.plugin.clone(), params, progress, incoming, self_incoming }));
            Some(base)
        }
    }
}

fn layer(track: &Track, c: &Arc<Clip>, t: Time) -> Layer {
    let clip_time = t - c.timeline_start;
    Layer {
        track: track.id,
        clip: c.id,
        source: c.source.clone(),
        source_time: c.source_time(t),
        clip_time,
        effects: c
            .effects
            .iter()
            .filter(|e| e.enabled)
            .map(|e| ResolvedEffect {
                plugin: e.plugin.clone(),
                params: e.params.iter().map(|(k, p)| (k.clone(), p.value_at(clip_time))).collect(),
            })
            .collect(),
        transition: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ve_time::TimeRange;

    fn s(x: i64) -> Time {
        Time::from_seconds(x)
    }

    #[test]
    fn layers_bottom_to_top_with_source_times() {
        let asset = AssetId::new();
        let mk = |start, src| {
            Arc::new(Clip {
                id: ClipId::new(),
                name: "c".into(),
                source: ClipSource::Asset { asset, audio_stream: 0 },
                source_range: TimeRange::new(s(src), s(5)),
                timeline_start: s(start),
                enabled: true,
                link: None,
                effects: Default::default(),
                retime: Default::default(),
                transition_in: None,
                transition_out: None,
                channels: Vec::new(),
            })
        };
        let mut v1 = Track::new(TrackKind::Video, "V1");
        v1.clips.push_back(mk(0, 10));
        let mut v2 = Track::new(TrackKind::Video, "V2");
        v2.clips.push_back(mk(2, 0));
        let mut hidden = Track::new(TrackKind::Video, "V3");
        hidden.enabled = false;
        hidden.clips.push_back(mk(0, 0));
        let a1 = Track::new(TrackKind::Audio, "A1");
        let seq = Sequence {
            id: SequenceId::new(),
            name: "s".into(),
            format: SequenceFormat::default(),
            tracks: [v1, v2, hidden, a1].into_iter().map(Arc::new).collect(),
            marks: Default::default(),
        };
        let plan = evaluate(&seq, s(3), Quality::PREVIEW);
        assert_eq!((plan.width, plan.height), (960, 540));
        assert_eq!(plan.layers.len(), 2, "disabled track and audio are not pictures");
        assert_eq!(plan.layers[0].source_time, s(13));
        assert_eq!(plan.layers[1].source_time, s(1));
        assert_eq!(plan.layers[1].clip_time, s(1));
        assert!(evaluate(&seq, s(9), Quality::FULL).layers.is_empty());
    }
}
