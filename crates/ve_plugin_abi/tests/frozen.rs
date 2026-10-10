//! ABI v1 is frozen: these are its layout and values, as numbers. A plugin
//! compiled against v1 relies on every one of them; changing any (moving a
//! field, resizing one, renumbering an enum) breaks every plugin shipped.
//! The C side asserts the same numbers at compile time (tests/layout.c).
//! Growing the ABI means appending fields after these, never changing them.

#![cfg(target_pointer_width = "64")]

use std::mem::{offset_of, size_of};
use ve_plugin_abi::*;

macro_rules! frozen {
    ($t:ty, size $size:expr; $($f:ident @ $off:expr),* $(,)?) => {
        assert_eq!(size_of::<$t>(), $size, concat!(stringify!($t), " size"));
        $( assert_eq!(offset_of!($t, $f), $off, concat!(stringify!($t), ".", stringify!($f))); )*
    };
}

#[test]
fn v1_layout_is_frozen() {
    frozen!(VeHost, size 24; struct_size @ 0, abi_version @ 4, log @ 8, get_extension @ 16);
    frozen!(VeParamDesc, size 88; struct_size @ 0, ty @ 4, id @ 8, label @ 16, min @ 24, max @ 32, default_value @ 40, choices @ 72, choice_count @ 80, flags @ 84);
    frozen!(VeParamValues, size 16; count @ 0, values @ 8);
    frozen!(VeImage, size 32; struct_size @ 0, width @ 4, height @ 8, stride @ 16, pixels @ 24);
    frozen!(VeRenderArgs, size 56; struct_size @ 0, time @ 8, frame_duration @ 16, scale @ 24, params @ 32, progress @ 48);
    frozen!(VeEffectDesc, size 96; struct_size @ 0, kind @ 4, id @ 8, name @ 16, category @ 24, major_version @ 32, minor_version @ 36, flags @ 40, params @ 48, param_count @ 56, wgsl @ 64, create @ 72, destroy @ 80, render_cpu @ 88);
    frozen!(VePluginDesc, size 40; struct_size @ 0, abi_version @ 4, name @ 8, vendor @ 16, effects @ 24, effect_count @ 32);
}

#[test]
fn v1_values_are_frozen() {
    assert_eq!(VE_ABI_VERSION, 1);
    assert_eq!([VeParamType::Bool, VeParamType::Int, VeParamType::Float, VeParamType::Vec2, VeParamType::Color, VeParamType::Choice].map(|t| t.0), [0, 1, 2, 3, 4, 5]);
    assert_eq!([VeEffectKind::Filter, VeEffectKind::Transition, VeEffectKind::Generator].map(|k| k.0), [0, 1, 2]);
    assert_eq!([VeLogLevel::Debug, VeLogLevel::Info, VeLogLevel::Warn, VeLogLevel::Error].map(|l| l.0), [0, 1, 2, 3]);
    assert_eq!(VE_PARAM_ANIMATABLE, 1);
    assert_eq!(entry_symbol(None), "ve_plugin_entry");
    assert_eq!(entry_symbol(Some("invert")), "ve_plugin_entry_invert");
}

#[test]
fn the_contract_document_holds_the_prelude_verbatim() {
    let doc = include_str!("../../../sdk/WGSL_CONTRACT.md");
    assert!(doc.contains(WGSL_PRELUDE), "sdk/WGSL_CONTRACT.md must contain ve_plugin_abi::WGSL_PRELUDE exactly");
    assert!(HEADER.contains("FROZEN"), "the header says it is frozen");
}
