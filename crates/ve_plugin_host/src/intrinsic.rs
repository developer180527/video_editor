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
/// The Color panel's grade: basic correction, creative, colour wheels.
pub const GRADE: &str = "ve.grade";

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

/// The grade, in the linear working space. Tone works in stops around mid
/// grey (0.18), so the controls feel the same on dark and bright shots;
/// the wheels push the shadows, midtones and highlights towards a hue.
/// Parameter order is the `params.values` index.
const GRADE_WGSL: &str = r#"
fn grade_luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2722287, 0.6740818, 0.0536895));
}
// A wheel's puck (x right, y up) as a colour offset with zero luma.
fn grade_wheel(w: vec2<f32>) -> vec3<f32> {
    let rgb = vec3<f32>(1.402 * w.y, -0.344136 * w.x - 0.714136 * w.y, 1.772 * w.x);
    return rgb - vec3<f32>(grade_luma(rgb));
}
@fragment
fn effect(i: EffectIn) -> @location(0) vec4<f32> {
    let s = textureSample(source, source_sampler, i.uv);
    let a = s.a;
    if (a <= 0.0) { return s; }
    var c = max(s.rgb / a, vec3<f32>(0.0));
    let v = params.values;
    // White balance: warm/cool along blue-amber, tint along green-magenta.
    let temp = v[0].x / 100.0;
    let tint = v[1].x / 100.0;
    c = c * vec3<f32>(1.0 + 0.3 * temp, 1.0 - 0.3 * tint, 1.0 - 0.3 * temp);
    // Exposure, in stops.
    c = c * exp2(v[2].x);
    // Tone, in stops from mid grey.
    let y0 = max(grade_luma(c), 1e-6);
    var st = log2(y0 / 0.18);
    st = st * (1.0 + v[3].x / 100.0);
    st = st + v[4].x / 100.0 * 1.5 * smoothstep(-0.5, 2.5, st);
    st = st + v[5].x / 100.0 * 1.5 * (1.0 - smoothstep(-4.0, 0.5, st));
    st = st + v[6].x / 100.0 * 1.0 * smoothstep(1.0, 3.5, st);
    st = st + v[7].x / 100.0 * 1.0 * (1.0 - smoothstep(-6.0, -2.5, st));
    c = c * (0.18 * exp2(st) / y0);
    // Wheels: lift the shadows, bend the midtones, scale the highlights.
    let y = clamp(grade_luma(c), 0.0, 1.0);
    let lift = (grade_wheel(v[10].xy) * 0.1 + vec3<f32>(v[11].x * 0.1)) * (1.0 - y);
    c = max(c + lift, vec3<f32>(0.0));
    let gamma = vec3<f32>(1.0) + grade_wheel(v[12].xy) * 0.5 + vec3<f32>(v[13].x * 0.5);
    c = pow(c, vec3<f32>(1.0) / max(gamma, vec3<f32>(0.05)));
    c = c * (vec3<f32>(1.0) + grade_wheel(v[14].xy) * 0.5 + vec3<f32>(v[15].x * 0.5));
    // Saturation, and vibrance: more on what is muted than what is vivid.
    let l = grade_luma(c);
    let hi = max(c.r, max(c.g, c.b));
    let lo = min(c.r, min(c.g, c.b));
    let chroma = select(0.0, (hi - lo) / hi, hi > 1e-6);
    let sat = v[8].x / 100.0 * (1.0 + v[9].x / 100.0 * (1.0 - chroma));
    c = max(vec3<f32>(l) + (c - vec3<f32>(l)) * sat, vec3<f32>(0.0));
    return vec4<f32>(c * a, a);
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
        EffectInfo {
            implementation: Implementation::ShaderOnly,
            wgsl: Some(GRADE_WGSL.into()),
            ..info(GRADE, "Lumetri Color", "Color Correction", vec![
                p("temperature", "Temperature", Float, -100.0, 100.0, [0.0; 4]),
                p("tint", "Tint", Float, -100.0, 100.0, [0.0; 4]),
                p("exposure", "Exposure", Float, -5.0, 5.0, [0.0; 4]),
                p("contrast", "Contrast", Float, -100.0, 100.0, [0.0; 4]),
                p("highlights", "Highlights", Float, -100.0, 100.0, [0.0; 4]),
                p("shadows", "Shadows", Float, -100.0, 100.0, [0.0; 4]),
                p("whites", "Whites", Float, -100.0, 100.0, [0.0; 4]),
                p("blacks", "Blacks", Float, -100.0, 100.0, [0.0; 4]),
                p("saturation", "Saturation", Float, 0.0, 200.0, [100.0, 0.0, 0.0, 0.0]),
                p("vibrance", "Vibrance", Float, -100.0, 100.0, [0.0; 4]),
                p("lift", "Shadows Color", Vec2, -1.0, 1.0, [0.0; 4]),
                p("lift_level", "Shadows Level", Float, -1.0, 1.0, [0.0; 4]),
                p("gamma", "Midtones Color", Vec2, -1.0, 1.0, [0.0; 4]),
                p("gamma_level", "Midtones Level", Float, -1.0, 1.0, [0.0; 4]),
                p("gain", "Highlights Color", Vec2, -1.0, 1.0, [0.0; 4]),
                p("gain_level", "Highlights Level", Float, -1.0, 1.0, [0.0; 4]),
            ])
        },
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
