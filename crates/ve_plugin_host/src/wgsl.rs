//! Plugin shaders are checked when the plugin loads, not when they first
//! render: a typo surfaces once, as a clear message naming the line, instead
//! of as a GPU error on every frame mid-edit.
//!
//! The check is the WGSL contract (sdk/WGSL_CONTRACT.md): the frozen
//! prelude followed by the plugin's source must parse and validate, and
//! must define `@fragment fn effect(...)` returning `@location(0)
//! vec4<f32>`. Line numbers in messages count from the start of the
//! plugin's own source.

use naga::{Binding, ScalarKind, ShaderStage, TypeInner, VectorSize};
use ve_plugin_abi::WGSL_PRELUDE;

/// Why a plugin shader was refused.
pub fn validate(source: &str) -> Result<(), String> {
    let full = format!("{WGSL_PRELUDE}{source}");
    let shift = WGSL_PRELUDE.lines().count();
    let module = naga::front::wgsl::parse_str(&full).map_err(|e| local_lines(&e.emit_to_string(&full), shift))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
        .validate(&module)
        .map_err(|e| local_lines(&e.emit_to_string(&full), shift))?;
    let Some(ep) = module.entry_points.iter().find(|e| e.name == "effect") else {
        return Err("no `effect` entry point: write `@fragment fn effect(i: EffectIn) -> @location(0) vec4<f32>`".into());
    };
    if ep.stage != ShaderStage::Fragment {
        return Err("`effect` must be a @fragment entry point".into());
    }
    let ok = ep.function.result.as_ref().is_some_and(|r| {
        matches!(r.binding, Some(Binding::Location { location: 0, .. }))
            && matches!(module.types[r.ty].inner, TypeInner::Vector { size: VectorSize::Quad, scalar } if scalar.kind == ScalarKind::Float && scalar.width == 4)
    });
    if !ok {
        return Err("`effect` must return `@location(0) vec4<f32>`".into());
    }
    if let Some(bad) = module.entry_points.iter().find(|e| e.name.starts_with("ve_")) {
        return Err(format!("`{}`: names starting `ve_` are the host's", bad.name));
    }
    Ok(())
}

/// naga's message, with `wgsl:LINE:COL` positions moved back by the
/// prelude's `shift` lines, so they point into the plugin's own source.
fn local_lines(msg: &str, shift: usize) -> String {
    let mut out = String::new();
    let mut rest = msg;
    while let Some(i) = rest.find("wgsl:") {
        out.push_str(&rest[..i + 5]);
        rest = &rest[i + 5..];
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        match digits.parse::<usize>() {
            Ok(n) if n > shift => {
                out.push_str(&(n - shift).to_string());
                rest = &rest[digits.len()..];
            }
            _ => {}
        }
    }
    out.push_str(rest);
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = "@fragment\nfn effect(i: EffectIn) -> @location(0) vec4<f32> {\n    return textureSample(source, source_sampler, i.uv) * params.values[0].x;\n}\n";

    #[test]
    fn a_good_shader_passes() {
        assert_eq!(validate(GOOD), Ok(()));
    }

    #[test]
    fn every_built_in_shader_passes() {
        for e in crate::intrinsic::all() {
            if let Some(w) = &e.wgsl {
                assert_eq!(validate(w), Ok(()), "{}", e.plugin.id);
            }
        }
    }

    #[test]
    fn mistakes_are_refused_with_the_plugins_own_line() {
        // A typo on line 3 of the plugin's source.
        let typo = "@fragment\nfn effect(i: EffectIn) -> @location(0) vec4<f32> {\n    return textureSample(sauce, source_sampler, i.uv);\n}\n";
        let e = validate(typo).unwrap_err();
        assert!(e.contains("sauce"), "{e}");
        assert!(e.contains("wgsl:3:"), "points at line 3 of the plugin: {e}");
        // The wrong shape.
        assert!(validate("@fragment\nfn main() -> @location(0) vec4<f32> { return vec4<f32>(0.0); }\n").unwrap_err().contains("no `effect`"));
        assert!(validate("@fragment\nfn effect(i: EffectIn) -> @location(0) vec2<f32> { return vec2<f32>(0.0); }\n").unwrap_err().contains("vec4"));
        // Type errors are caught by validation, not just parsing.
        let types = "@fragment\nfn effect(i: EffectIn) -> @location(0) vec4<f32> {\n    let x: f32 = params.values[0];\n    return vec4<f32>(x);\n}\n";
        assert!(validate(types).is_err());
    }
}
