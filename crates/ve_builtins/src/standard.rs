//! The standard pack: 40 effects and 25 transitions, written as one ABI v1
//! plugin exactly as a third party would write one. Its descriptor is built
//! once, on the first call of its entry, and lives for the program (the
//! ABI's rule for a plugin's static data); the host reads it with
//! `native::load`, validating every shader on the way in.
//!
//! Units: sizes are in the picture's pixels (shaders multiply by
//! `params.scale`); points are percent of the frame, so they fit any
//! resolution; tone controls work on display-encoded values.

use std::ffi::{c_char, CString};
use std::sync::OnceLock;
use ve_plugin_abi::*;

/// Helpers every shader in the pack starts with.
const COMMON: &str = include_str!("../shaders/common.wgsl");

macro_rules! fx {
    ($name:literal) => {
        include_str!(concat!("../shaders/fx/", $name, ".wgsl"))
    };
}
macro_rules! tr {
    ($name:literal) => {
        include_str!(concat!("../shaders/tr/", $name, ".wgsl"))
    };
}

/// One parameter, as declared.
struct P {
    ty: VeParamType,
    id: &'static str,
    label: &'static str,
    min: f64,
    max: f64,
    default: [f64; 4],
    choices: &'static [&'static str],
    animatable: bool,
}

fn float(id: &'static str, label: &'static str, min: f64, max: f64, default: f64) -> P {
    P { ty: VeParamType::Float, id, label, min, max, default: [default, 0.0, 0.0, 0.0], choices: &[], animatable: true }
}

fn int(id: &'static str, label: &'static str, min: f64, max: f64, default: f64) -> P {
    P { ty: VeParamType::Int, ..float(id, label, min, max, default) }
}

/// A position, in percent of the frame.
fn point(id: &'static str, label: &'static str, x: f64, y: f64) -> P {
    P { ty: VeParamType::Vec2, default: [x, y, 0.0, 0.0], ..float(id, label, -100.0, 200.0, 0.0) }
}

/// A colour as the picker shows it (display-encoded).
fn color(id: &'static str, label: &'static str, rgba: [f64; 4]) -> P {
    P { ty: VeParamType::Color, default: rgba, ..float(id, label, 0.0, 1.0, 0.0) }
}

fn toggle(id: &'static str, label: &'static str, on: bool) -> P {
    P { ty: VeParamType::Bool, animatable: false, ..float(id, label, 0.0, 1.0, on as u8 as f64) }
}

fn pick(id: &'static str, label: &'static str, choices: &'static [&'static str], default: usize) -> P {
    P { ty: VeParamType::Choice, choices, animatable: false, ..float(id, label, 0.0, (choices.len() - 1) as f64, default as f64) }
}

/// Softness of a transition's edge, in percent of its travel.
fn softness(default: f64) -> P {
    float("softness", "Softness", 0.0, 100.0, default)
}

const DIRECTIONS: &[&str] = &["From Left", "From Right", "From Top", "From Bottom"];

/// One effect, as declared.
struct Fx {
    kind: VeEffectKind,
    id: &'static str,
    name: &'static str,
    category: &'static str,
    params: Vec<P>,
    wgsl: &'static str,
}

fn filter(id: &'static str, name: &'static str, category: &'static str, wgsl: &'static str, params: Vec<P>) -> Fx {
    Fx { kind: VeEffectKind::Filter, id, name, category, params, wgsl }
}

fn transition(id: &'static str, name: &'static str, category: &'static str, wgsl: &'static str, params: Vec<P>) -> Fx {
    Fx { kind: VeEffectKind::Transition, ..filter(id, name, category, wgsl, params) }
}

/// Everything in the pack.
fn all() -> Vec<Fx> {
    const CC: &str = "Color Correction";
    const BLUR: &str = "Blur & Sharpen";
    const DISTORT: &str = "Distort";
    const STYLIZE: &str = "Stylize";
    const KEY: &str = "Keying";
    let white = [1.0, 1.0, 1.0, 1.0];
    let black = [0.0, 0.0, 0.0, 1.0];
    let channel = |id, label, d| float(id, label, -200.0, 200.0, d);
    let balance = |id, label| float(id, label, -100.0, 100.0, 0.0);
    let centre = || point("center", "Center", 50.0, 50.0);
    vec![
        // Color correction: neutral at their defaults.
        filter("brightness_contrast", "Brightness & Contrast", CC, fx!("brightness_contrast"), vec![
            float("brightness", "Brightness", -100.0, 100.0, 0.0),
            float("contrast", "Contrast", -100.0, 100.0, 0.0),
        ]),
        filter("levels", "Levels", CC, fx!("levels"), vec![
            float("input_black", "Input Black", 0.0, 255.0, 0.0),
            float("input_white", "Input White", 0.0, 255.0, 255.0),
            float("gamma", "Gamma", 0.1, 10.0, 1.0),
            float("output_black", "Output Black", 0.0, 255.0, 0.0),
            float("output_white", "Output White", 0.0, 255.0, 255.0),
        ]),
        filter("hue_saturation", "Hue/Saturation", CC, fx!("hue_saturation"), vec![
            float("hue", "Hue", -180.0, 180.0, 0.0),
            float("saturation", "Saturation", 0.0, 200.0, 100.0),
            float("lightness", "Lightness", -100.0, 100.0, 0.0),
        ]),
        filter("channel_mixer", "Channel Mixer", CC, fx!("channel_mixer"), vec![
            channel("red_red", "Red-Red", 100.0),
            channel("red_green", "Red-Green", 0.0),
            channel("red_blue", "Red-Blue", 0.0),
            channel("green_red", "Green-Red", 0.0),
            channel("green_green", "Green-Green", 100.0),
            channel("green_blue", "Green-Blue", 0.0),
            channel("blue_red", "Blue-Red", 0.0),
            channel("blue_green", "Blue-Green", 0.0),
            channel("blue_blue", "Blue-Blue", 100.0),
            toggle("monochrome", "Monochrome", false),
        ]),
        filter("color_balance", "Color Balance", CC, fx!("color_balance"), vec![
            balance("shadow_red", "Shadow Red"),
            balance("shadow_green", "Shadow Green"),
            balance("shadow_blue", "Shadow Blue"),
            balance("midtone_red", "Midtone Red"),
            balance("midtone_green", "Midtone Green"),
            balance("midtone_blue", "Midtone Blue"),
            balance("highlight_red", "Highlight Red"),
            balance("highlight_green", "Highlight Green"),
            balance("highlight_blue", "Highlight Blue"),
            toggle("preserve_luminosity", "Preserve Luminosity", true),
        ]),
        filter("tint", "Tint", CC, fx!("tint"), vec![
            color("map_black", "Map Black To", black),
            color("map_white", "Map White To", white),
            float("amount", "Amount to Tint", 0.0, 100.0, 100.0),
        ]),
        filter("black_white", "Black & White", CC, fx!("black_white"), vec![
            float("reds", "Reds", -200.0, 300.0, 40.0),
            float("yellows", "Yellows", -200.0, 300.0, 60.0),
            float("greens", "Greens", -200.0, 300.0, 40.0),
            float("cyans", "Cyans", -200.0, 300.0, 60.0),
            float("blues", "Blues", -200.0, 300.0, 20.0),
            float("magentas", "Magentas", -200.0, 300.0, 80.0),
            toggle("tint", "Tint", false),
            color("tint_color", "Tint Color", [0.88, 0.78, 0.62, 1.0]),
        ]),
        filter("photo_filter", "Photo Filter", CC, fx!("photo_filter"), vec![
            color("color", "Color", [0.925, 0.541, 0.0, 1.0]),
            float("density", "Density", 0.0, 100.0, 25.0),
            toggle("preserve_luminosity", "Preserve Luminosity", true),
        ]),
        filter("leave_color", "Leave Color", CC, fx!("leave_color"), vec![
            color("color", "Color to Leave", [0.85, 0.1, 0.1, 1.0]),
            float("tolerance", "Tolerance", 0.0, 100.0, 15.0),
            float("softness", "Edge Softness", 0.0, 100.0, 10.0),
            float("amount", "Amount to Decolor", 0.0, 100.0, 100.0),
        ]),
        filter("change_to_color", "Change to Color", CC, fx!("change_to_color"), vec![
            color("from", "From", [0.85, 0.1, 0.1, 1.0]),
            color("to", "To", [0.1, 0.3, 0.85, 1.0]),
            pick("change", "Change", &["Hue", "Hue & Lightness", "Hue, Lightness & Saturation"], 0),
            float("tolerance", "Tolerance", 0.0, 100.0, 15.0),
            float("softness", "Softness", 0.0, 100.0, 10.0),
        ]),
        filter("posterize", "Posterize", CC, fx!("posterize"), vec![int("levels", "Level", 2.0, 64.0, 7.0)]),
        filter("threshold", "Threshold", CC, fx!("threshold"), vec![
            float("level", "Level", 0.0, 255.0, 128.0),
            float("softness", "Softness", 0.0, 100.0, 0.0),
        ]),
        // Blur and sharpen.
        filter("gaussian_blur", "Gaussian Blur", BLUR, fx!("gaussian_blur"), vec![
            float("blurriness", "Blurriness", 0.0, 1000.0, 10.0),
            pick("dimensions", "Blur Dimensions", &["Horizontal and Vertical", "Horizontal", "Vertical"], 0),
            toggle("repeat_edges", "Repeat Edge Pixels", false),
        ]),
        filter("directional_blur", "Directional Blur", BLUR, fx!("directional_blur"), vec![
            float("direction", "Direction", -360.0, 360.0, 0.0),
            float("length", "Blur Length", 0.0, 1000.0, 10.0),
        ]),
        filter("radial_blur", "Radial Blur", BLUR, fx!("radial_blur"), vec![
            float("amount", "Amount", 0.0, 100.0, 10.0),
            pick("type", "Type", &["Spin", "Zoom"], 1),
            centre(),
        ]),
        filter("sharpen", "Sharpen", BLUR, fx!("sharpen"), vec![float("amount", "Sharpen Amount", 0.0, 500.0, 50.0)]),
        filter("unsharp_mask", "Unsharp Mask", BLUR, fx!("unsharp_mask"), vec![
            float("amount", "Amount", 0.0, 500.0, 50.0),
            float("radius", "Radius", 0.0, 250.0, 2.0),
            float("threshold", "Threshold", 0.0, 255.0, 0.0),
        ]),
        // Distort.
        filter("mirror", "Mirror", DISTORT, fx!("mirror"), vec![
            point("center", "Reflection Center", 50.0, 50.0),
            float("angle", "Reflection Angle", -360.0, 360.0, 0.0),
        ]),
        filter("corner_pin", "Corner Pin", DISTORT, fx!("corner_pin"), vec![
            point("upper_left", "Upper Left", 0.0, 0.0),
            point("upper_right", "Upper Right", 100.0, 0.0),
            point("lower_left", "Lower Left", 0.0, 100.0),
            point("lower_right", "Lower Right", 100.0, 100.0),
        ]),
        filter("lens_distortion", "Lens Distortion", DISTORT, fx!("lens_distortion"), vec![
            float("curvature", "Curvature", -100.0, 100.0, -20.0),
            centre(),
        ]),
        filter("wave_warp", "Wave Warp", DISTORT, fx!("wave_warp"), vec![
            pick("type", "Wave Type", &["Sine", "Square", "Triangle", "Sawtooth"], 0),
            float("height", "Wave Height", 0.0, 1000.0, 10.0),
            float("width", "Wave Width", 1.0, 2000.0, 40.0),
            float("direction", "Direction", -360.0, 360.0, 90.0),
            float("speed", "Wave Speed", -10.0, 10.0, 1.0),
            float("phase", "Phase", -360.0, 360.0, 0.0),
        ]),
        filter("twirl", "Twirl", DISTORT, fx!("twirl"), vec![
            float("angle", "Angle", -3600.0, 3600.0, 90.0),
            float("radius", "Twirl Radius", 0.0, 200.0, 30.0),
            centre(),
        ]),
        filter("spherize", "Spherize", DISTORT, fx!("spherize"), vec![
            float("amount", "Amount", -100.0, 100.0, 50.0),
            float("radius", "Radius", 0.0, 200.0, 30.0),
            centre(),
        ]),
        filter("ripple", "Ripple", DISTORT, fx!("ripple"), vec![
            centre(),
            float("amplitude", "Amplitude", 0.0, 500.0, 10.0),
            float("wavelength", "Wavelength", 1.0, 2000.0, 60.0),
            float("speed", "Speed", -10.0, 10.0, 1.0),
            float("damping", "Damping", 0.0, 100.0, 50.0),
        ]),
        filter("turbulent_displace", "Turbulent Displace", DISTORT, fx!("turbulent_displace"), vec![
            float("amount", "Amount", 0.0, 1000.0, 50.0),
            float("size", "Size", 2.0, 1000.0, 100.0),
            int("complexity", "Complexity", 1.0, 8.0, 3.0),
            float("evolution", "Evolution", -36000.0, 36000.0, 0.0),
        ]),
        // Transform.
        filter("flip", "Flip", "Transform", fx!("flip"), vec![toggle("horizontal", "Horizontal", true), toggle("vertical", "Vertical", false)]),
        filter("edge_feather", "Edge Feather", "Transform", fx!("edge_feather"), vec![float("amount", "Amount", 0.0, 500.0, 20.0)]),
        // Stylize.
        filter("glow", "Glow", STYLIZE, fx!("glow"), vec![
            float("threshold", "Threshold", 0.0, 100.0, 80.0),
            float("radius", "Radius", 0.0, 500.0, 20.0),
            float("intensity", "Intensity", 0.0, 400.0, 100.0),
            color("color", "Color", white),
        ]),
        filter("mosaic", "Mosaic", STYLIZE, fx!("mosaic"), vec![float("size", "Block Size", 1.0, 500.0, 20.0), toggle("sharp", "Sharp Colors", false)]),
        filter("emboss", "Emboss", STYLIZE, fx!("emboss"), vec![
            float("direction", "Direction", -360.0, 360.0, 45.0),
            float("relief", "Relief", 0.5, 10.0, 2.0),
            float("contrast", "Contrast", 0.0, 500.0, 100.0),
            float("blend", "Blend With Original", 0.0, 100.0, 0.0),
        ]),
        filter("find_edges", "Find Edges", STYLIZE, fx!("find_edges"), vec![
            toggle("invert", "Invert", false),
            float("blend", "Blend With Original", 0.0, 100.0, 0.0),
        ]),
        filter("noise", "Noise", STYLIZE, fx!("noise"), vec![
            float("amount", "Amount of Noise", 0.0, 100.0, 20.0),
            toggle("color", "Color Noise", true),
            float("size", "Grain Size", 1.0, 20.0, 1.0),
            toggle("animated", "Animated", true),
        ]),
        filter("chromatic_aberration", "Chromatic Aberration", STYLIZE, fx!("chromatic_aberration"), vec![
            float("amount", "Amount", 0.0, 200.0, 5.0),
            centre(),
        ]),
        filter("kaleidoscope", "Kaleidoscope", STYLIZE, fx!("kaleidoscope"), vec![
            int("segments", "Segments", 2.0, 32.0, 6.0),
            float("angle", "Angle", -360.0, 360.0, 0.0),
            centre(),
        ]),
        filter("vignette", "Vignette", STYLIZE, fx!("vignette"), vec![
            float("amount", "Amount", -100.0, 100.0, -50.0),
            float("midpoint", "Midpoint", 0.0, 100.0, 50.0),
            float("roundness", "Roundness", -100.0, 100.0, 0.0),
            float("feather", "Feather", 0.0, 100.0, 50.0),
        ]),
        filter("drop_shadow", "Drop Shadow", "Perspective", fx!("drop_shadow"), vec![
            color("color", "Shadow Color", black),
            float("opacity", "Opacity", 0.0, 100.0, 50.0),
            float("direction", "Direction", -360.0, 360.0, 135.0),
            float("distance", "Distance", 0.0, 1000.0, 10.0),
            float("softness", "Softness", 0.0, 500.0, 10.0),
            toggle("shadow_only", "Shadow Only", false),
        ]),
        // Keying.
        filter("chroma_key", "Chroma Key", KEY, fx!("chroma_key"), vec![
            color("key", "Key Color", [0.1, 0.8, 0.2, 1.0]),
            float("tolerance", "Tolerance", 0.0, 100.0, 30.0),
            float("softness", "Softness", 0.0, 100.0, 15.0),
            float("spill", "Spill Suppression", 0.0, 100.0, 50.0),
            pick("output", "Output", &["Composite", "Alpha Channel"], 0),
        ]),
        filter("luma_key", "Luma Key", KEY, fx!("luma_key"), vec![
            float("threshold", "Threshold", 0.0, 100.0, 10.0),
            float("softness", "Softness", 0.0, 100.0, 10.0),
            toggle("invert", "Invert", false),
        ]),
        filter("garbage_matte", "Garbage Matte", KEY, fx!("garbage_matte"), vec![
            point("upper_left", "Upper Left", 10.0, 10.0),
            point("upper_right", "Upper Right", 90.0, 10.0),
            point("lower_left", "Lower Left", 10.0, 90.0),
            point("lower_right", "Lower Right", 90.0, 90.0),
            float("feather", "Feather", 0.0, 500.0, 0.0),
            toggle("invert", "Invert", false),
        ]),
        // A generator.
        Fx {
            kind: VeEffectKind::Generator,
            ..filter("gradient", "Gradient", "Generators", fx!("gradient"), vec![
                point("start", "Start of Ramp", 50.0, 0.0),
                color("start_color", "Start Color", black),
                point("end", "End of Ramp", 50.0, 100.0),
                color("end_color", "End Color", white),
                pick("shape", "Ramp Shape", &["Linear Ramp", "Radial Ramp"], 0),
            ])
        },
        // Transitions.
        transition("dip_to_white", "Dip to White", "Dissolve", tr!("dip_to_white"), vec![]),
        transition("additive_dissolve", "Additive Dissolve", "Dissolve", tr!("additive_dissolve"), vec![]),
        transition("non_additive_dissolve", "Non-Additive Dissolve", "Dissolve", tr!("non_additive_dissolve"), vec![]),
        transition("luma_fade", "Luma Fade", "Dissolve", tr!("luma_fade"), vec![softness(20.0), toggle("invert", "Invert", false)]),
        transition("blur_dissolve", "Blur Dissolve", "Dissolve", tr!("blur_dissolve"), vec![float("blur", "Blur", 0.0, 500.0, 40.0)]),
        transition("flash", "Flash", "Dissolve", tr!("flash"), vec![float("intensity", "Intensity", 0.0, 10.0, 3.0)]),
        transition("wipe", "Wipe", "Wipe", tr!("wipe"), vec![float("direction", "Direction", -360.0, 360.0, 0.0), softness(0.0)]),
        transition("barn_doors", "Barn Doors", "Wipe", tr!("barn_doors"), vec![pick("orientation", "Orientation", &["Vertical", "Horizontal"], 0), softness(0.0)]),
        transition("clock_wipe", "Clock Wipe", "Wipe", tr!("clock_wipe"), vec![pick("direction", "Direction", &["Clockwise", "Counter-Clockwise"], 0), softness(0.0)]),
        transition("venetian_blinds", "Venetian Blinds", "Wipe", tr!("venetian_blinds"), vec![
            int("slats", "Slats", 1.0, 100.0, 10.0),
            pick("orientation", "Orientation", &["Horizontal", "Vertical"], 0),
            softness(0.0),
        ]),
        transition("checker_wipe", "Checker Wipe", "Wipe", tr!("checker_wipe"), vec![int("squares", "Squares Across", 1.0, 100.0, 8.0), softness(0.0)]),
        transition("random_blocks", "Random Blocks", "Wipe", tr!("random_blocks"), vec![int("blocks", "Blocks Across", 1.0, 200.0, 16.0), softness(5.0)]),
        transition("band_wipe", "Band Wipe", "Wipe", tr!("band_wipe"), vec![
            int("bands", "Bands", 1.0, 100.0, 7.0),
            pick("orientation", "Orientation", &["Horizontal", "Vertical"], 0),
            softness(0.0),
        ]),
        transition("inset", "Inset", "Wipe", tr!("inset"), vec![
            pick("corner", "Corner", &["Upper Left", "Upper Right", "Lower Left", "Lower Right"], 0),
            softness(0.0),
        ]),
        transition("iris_round", "Iris Round", "Iris", tr!("iris_round"), vec![centre(), softness(0.0)]),
        transition("iris_box", "Iris Box", "Iris", tr!("iris_box"), vec![centre(), softness(0.0)]),
        transition("iris_diamond", "Iris Diamond", "Iris", tr!("iris_diamond"), vec![centre(), softness(0.0)]),
        transition("iris_cross", "Iris Cross", "Iris", tr!("iris_cross"), vec![centre(), softness(0.0)]),
        transition("push", "Push", "Slide", tr!("push"), vec![pick("direction", "Direction", DIRECTIONS, 0)]),
        transition("slide", "Slide", "Slide", tr!("slide"), vec![pick("direction", "Direction", DIRECTIONS, 0)]),
        transition("split", "Split", "Slide", tr!("split"), vec![pick("orientation", "Orientation", &["Horizontal", "Vertical"], 0)]),
        transition("whip", "Whip", "Slide", tr!("whip"), vec![pick("direction", "Direction", DIRECTIONS, 0), float("blur", "Motion Blur", 0.0, 100.0, 60.0)]),
        transition("cross_zoom", "Cross Zoom", "Zoom & 3D", tr!("cross_zoom"), vec![float("strength", "Strength", 0.0, 100.0, 50.0)]),
        transition("flip_over", "Flip Over", "Zoom & 3D", tr!("flip_over"), vec![
            pick("axis", "Axis", &["Vertical", "Horizontal"], 0),
            float("perspective", "Perspective", 0.0, 100.0, 50.0),
        ]),
        transition("pixelate", "Pixelate", "Zoom & 3D", tr!("pixelate"), vec![float("size", "Largest Block", 2.0, 500.0, 60.0)]),
    ]
}

/// A string for the descriptor, alive for the program.
fn leak(s: impl Into<Vec<u8>>) -> *const c_char {
    CString::new(s).expect("no NUL inside").into_raw()
}

fn param_desc(p: &P) -> VeParamDesc {
    let choices: &'static [*const c_char] = Box::leak(p.choices.iter().map(|c| leak(*c)).collect::<Box<[_]>>());
    VeParamDesc {
        struct_size: std::mem::size_of::<VeParamDesc>() as u32,
        ty: p.ty,
        id: leak(p.id),
        label: leak(p.label),
        min: p.min,
        max: p.max,
        default_value: p.default,
        choices: if choices.is_empty() { std::ptr::null() } else { choices.as_ptr() },
        choice_count: choices.len() as u32,
        flags: if p.animatable { VE_PARAM_ANIMATABLE } else { 0 },
    }
}

fn effect_desc(f: &Fx) -> &'static VeEffectDesc {
    let params: &'static [VeParamDesc] = Box::leak(f.params.iter().map(param_desc).collect::<Box<[_]>>());
    Box::leak(Box::new(VeEffectDesc {
        struct_size: std::mem::size_of::<VeEffectDesc>() as u32,
        kind: f.kind,
        id: leak(format!("{ID_PREFIX}{}", f.id)),
        name: leak(f.name),
        category: leak(f.category),
        major_version: 1,
        minor_version: 0,
        flags: 0,
        params: if params.is_empty() { std::ptr::null() } else { params.as_ptr() },
        param_count: params.len() as u32,
        wgsl: leak(format!("{COMMON}\n{}", f.wgsl)),
        create: None,
        destroy: None,
        render_cpu: None,
    }))
}

/// Every id in the pack starts with this.
pub const ID_PREFIX: &str = "org.ve.std.";

/// The pack's entry (`ve_plugin_entry_standard`).
///
/// # Safety
/// `host` is null or points to a `VeHost`.
pub unsafe extern "C" fn ve_plugin_entry_standard(host: *const VeHost) -> *const VePluginDesc {
    if host.is_null() || unsafe { (*host).abi_version } < 1 {
        return std::ptr::null();
    }
    // Held as an address: raw pointers are not `Sync`.
    static DESC: OnceLock<usize> = OnceLock::new();
    *DESC.get_or_init(|| {
        let effects: &'static [*const VeEffectDesc] = Box::leak(all().iter().map(|f| effect_desc(f) as *const VeEffectDesc).collect::<Box<[_]>>());
        let desc: &'static VePluginDesc = Box::leak(Box::new(VePluginDesc {
            struct_size: std::mem::size_of::<VePluginDesc>() as u32,
            abi_version: VE_ABI_VERSION,
            name: leak("Standard Effects"),
            vendor: leak("ve"),
            effects: effects.as_ptr(),
            effect_count: effects.len() as u32,
        }));
        desc as *const VePluginDesc as usize
    }) as *const VePluginDesc
}
