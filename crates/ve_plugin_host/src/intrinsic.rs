//! Effects every clip has: Motion and Opacity on video, Volume and Panner on
//! audio. They are described exactly like plugin effects (so Effect Controls,
//! keyframing and undo treat them the same), but the compositor and mixer
//! implement them directly.

use ve_model::{PluginApi, PluginRef};

use crate::{EffectInfo, EffectKind, Implementation, ParamInfo, ParamKind};

pub const MOTION: &str = "ve.motion";
pub const OPACITY: &str = "ve.opacity";
pub const VOLUME: &str = "ve.volume";
pub const PANNER: &str = "ve.panner";

pub fn plugin_ref(id: &str) -> PluginRef {
    PluginRef { api: PluginApi::Builtin, id: id.into(), major_version: 1 }
}

fn p(id: &str, label: &str, kind: ParamKind, min: f64, max: f64, default: [f64; 4]) -> ParamInfo {
    ParamInfo { id: id.into(), label: label.into(), kind, min, max, default, animatable: true }
}

fn info(id: &str, name: &str, category: &str, params: Vec<ParamInfo>) -> EffectInfo {
    EffectInfo {
        plugin: plugin_ref(id),
        name: name.into(),
        category: category.into(),
        kind: EffectKind::Filter,
        params,
        wgsl: None,
        implementation: Implementation::Intrinsic,
    }
}

/// The intrinsic effects. Position and Anchor Point defaults are filled in
/// per clip from the sequence and source sizes when a clip is made.
pub fn all() -> Vec<EffectInfo> {
    use ParamKind::*;
    let inf = f64::INFINITY;
    vec![
        info(MOTION, "Motion", "Intrinsic", vec![
            p("position", "Position", Vec2, -inf, inf, [960.0, 540.0, 0.0, 0.0]),
            p("scale", "Scale", Float, 0.0, 10000.0, [100.0, 0.0, 0.0, 0.0]),
            p("rotation", "Rotation", Float, -inf, inf, [0.0; 4]),
            p("anchor", "Anchor Point", Vec2, -inf, inf, [960.0, 540.0, 0.0, 0.0]),
            p("anti_flicker", "Anti-flicker Filter", Float, 0.0, 1.0, [0.0; 4]),
            p("crop_left", "Crop Left", Float, 0.0, 100.0, [0.0; 4]),
            p("crop_top", "Crop Top", Float, 0.0, 100.0, [0.0; 4]),
            p("crop_right", "Crop Right", Float, 0.0, 100.0, [0.0; 4]),
            p("crop_bottom", "Crop Bottom", Float, 0.0, 100.0, [0.0; 4]),
        ]),
        info(OPACITY, "Opacity", "Intrinsic", vec![
            p("opacity", "Opacity", Float, 0.0, 100.0, [100.0, 0.0, 0.0, 0.0]),
            ParamInfo {
                animatable: false,
                ..p("blend", "Blend Mode", Choice(["Normal", "Multiply", "Screen", "Add", "Overlay"].map(String::from).to_vec()), 0.0, 4.0, [0.0; 4])
            },
        ]),
        info(VOLUME, "Volume", "Intrinsic", vec![p("level", "Level", Float, -96.0, 15.0, [0.0; 4])]),
        info(PANNER, "Panner", "Intrinsic", vec![p("balance", "Balance", Float, -100.0, 100.0, [0.0; 4])]),
    ]
}
