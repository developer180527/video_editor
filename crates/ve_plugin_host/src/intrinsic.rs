//! Effects every clip has — Motion and Opacity on video, Volume and Panner on
//! audio — and the built-in transitions and generators. They are described
//! exactly like plugin effects (so Effect Controls, keyframing and undo treat
//! them the same). Dissolve and Dip to Black are WGSL like any plugin
//! transition; the rest the engine implements itself.

use ve_model::{PluginApi, PluginRef};

use crate::{EffectInfo, EffectKind, Implementation, ParamInfo, ParamKind};

pub const MOTION: &str = "ve.motion";
pub const OPACITY: &str = "ve.opacity";
pub const VOLUME: &str = "ve.volume";
pub const PANNER: &str = "ve.panner";
pub const DISSOLVE: &str = "ve.dissolve";
pub const DIP_TO_BLACK: &str = "ve.dip_to_black";
pub const CROSSFADE: &str = "ve.crossfade";
pub const COLOR_MATTE: &str = "ve.color";
pub const BARS: &str = "ve.bars";
pub const TITLE: &str = "ve.title";

/// Cross dissolve: premultiplied, scene-linear, so it is exact light mixing.
const DISSOLVE_WGSL: &str = r#"
@fragment
fn effect(i: EffectIn) -> @location(0) vec4<f32> {
    let a = textureSample(source, source_sampler, i.uv);
    let b = textureSample(source_b, source_sampler, i.uv);
    return mix(a, b, params.progress);
}
"#;

/// Dip to black: the first half fades out to black, the second fades in.
const DIP_WGSL: &str = r#"
@fragment
fn effect(i: EffectIn) -> @location(0) vec4<f32> {
    let p = params.progress;
    let a = textureSample(source, source_sampler, i.uv);
    let b = textureSample(source_b, source_sampler, i.uv);
    let black = vec4<f32>(0.0, 0.0, 0.0, max(a.a, b.a));
    if (p < 0.5) { return mix(a, black, p * 2.0); }
    return mix(black, b, p * 2.0 - 1.0);
}
"#;

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

fn transition(id: &str, name: &str, wgsl: Option<&str>) -> EffectInfo {
    EffectInfo {
        kind: EffectKind::Transition,
        wgsl: wgsl.map(String::from),
        implementation: if wgsl.is_some() { Implementation::ShaderOnly } else { Implementation::Intrinsic },
        ..info(id, name, "Transitions", vec![])
    }
}

fn generator(id: &str, name: &str, params: Vec<ParamInfo>) -> EffectInfo {
    EffectInfo { kind: EffectKind::Generator, ..info(id, name, "Generators", params) }
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
        transition(DISSOLVE, "Cross Dissolve", Some(DISSOLVE_WGSL)),
        transition(DIP_TO_BLACK, "Dip to Black", Some(DIP_WGSL)),
        // Audio: equal power, done by the mixer.
        transition(CROSSFADE, "Constant Power", None),
        // Colours are display-referred (what a colour picker shows); the
        // generators draw in display space, so their pictures take the same
        // colour path as footage.
        generator(COLOR_MATTE, "Color Matte", vec![p("color", "Color", Color, 0.0, 1.0, [0.5, 0.5, 0.5, 1.0])]),
        generator(BARS, "Bars", vec![]),
        generator(TITLE, "Title", vec![
            ParamInfo { animatable: false, ..p("text", "Text", Text("Title".into()), 0.0, 0.0, [0.0; 4]) },
            p("size", "Size", Float, 1.0, 1000.0, [96.0, 0.0, 0.0, 0.0]),
            p("color", "Color", Color, 0.0, 1.0, [1.0; 4]),
            p("position", "Position", Vec2, -inf, inf, [960.0, 540.0, 0.0, 0.0]),
        ]),
    ]
}
