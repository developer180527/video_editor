//! Rust mirror of `include/ve_plugin.h`. The header is the source of truth;
//! `tests/layout.rs` checks every struct against the C compiler's layout.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_void};

pub const VE_ABI_VERSION: u32 = 1;
pub const VE_PARAM_ANIMATABLE: u32 = 1 << 0;
pub const VE_FLAG_THREAD_SAFE: u32 = 1 << 0;

/// The header, for tools that ship it (the SDK, the CLI's `sdk` command).
pub const HEADER: &str = include_str!("../include/ve_plugin.h");

/// A log message's level (`VeLogLevel` in C). A plain `u32`, not a Rust enum: a newer plugin may send a value
/// this host does not know, which must be refused, not undefined behaviour.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VeLogLevel(pub u32);

#[allow(non_upper_case_globals)]
impl VeLogLevel {
    pub const Debug: VeLogLevel = VeLogLevel(0);
    pub const Info: VeLogLevel = VeLogLevel(1);
    pub const Warn: VeLogLevel = VeLogLevel(2);
    pub const Error: VeLogLevel = VeLogLevel(3);
}

#[repr(C)]
pub struct VeHost {
    pub struct_size: u32,
    pub abi_version: u32,
    pub log: Option<unsafe extern "C" fn(VeLogLevel, *const c_char)>,
    pub get_extension: Option<unsafe extern "C" fn(*const c_char) -> *const c_void>,
}

/// A parameter's type (`VeParamType` in C). A plain `u32`, not a Rust enum: a newer plugin may send a value
/// this host does not know, which must be refused, not undefined behaviour.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VeParamType(pub u32);

#[allow(non_upper_case_globals)]
impl VeParamType {
    pub const Bool: VeParamType = VeParamType(0);
    pub const Int: VeParamType = VeParamType(1);
    pub const Float: VeParamType = VeParamType(2);
    pub const Vec2: VeParamType = VeParamType(3);
    pub const Color: VeParamType = VeParamType(4);
    pub const Choice: VeParamType = VeParamType(5);
}

#[repr(C)]
pub struct VeParamDesc {
    pub struct_size: u32,
    pub ty: VeParamType,
    pub id: *const c_char,
    pub label: *const c_char,
    pub min: f64,
    pub max: f64,
    pub default_value: [f64; 4],
    pub choices: *const *const c_char,
    pub choice_count: u32,
    pub flags: u32,
}

#[repr(C)]
pub struct VeParamValues {
    pub count: u32,
    pub values: *const [f64; 4],
}

#[repr(C)]
pub struct VeImage {
    pub struct_size: u32,
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub pixels: *mut f32,
}

#[repr(C)]
pub struct VeRenderArgs {
    pub struct_size: u32,
    pub time: f64,
    pub frame_duration: f64,
    pub scale: f64,
    pub params: VeParamValues,
    pub progress: f64,
}

/// What an effect is (`VeEffectKind` in C). A plain `u32`, not a Rust enum: a newer plugin may send a value
/// this host does not know, which must be refused, not undefined behaviour.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VeEffectKind(pub u32);

#[allow(non_upper_case_globals)]
impl VeEffectKind {
    pub const Filter: VeEffectKind = VeEffectKind(0);
    pub const Transition: VeEffectKind = VeEffectKind(1);
    pub const Generator: VeEffectKind = VeEffectKind(2);
}

pub type CreateFn = unsafe extern "C" fn(*const VeHost) -> *mut c_void;
pub type DestroyFn = unsafe extern "C" fn(*mut c_void);
pub type RenderCpuFn =
    unsafe extern "C" fn(*mut c_void, *const VeRenderArgs, *const *const VeImage, u32, *mut VeImage) -> i32;

#[repr(C)]
pub struct VeEffectDesc {
    pub struct_size: u32,
    pub kind: VeEffectKind,
    pub id: *const c_char,
    pub name: *const c_char,
    pub category: *const c_char,
    pub major_version: u32,
    pub minor_version: u32,
    pub flags: u32,
    pub params: *const VeParamDesc,
    pub param_count: u32,
    pub wgsl: *const c_char,
    pub create: Option<CreateFn>,
    pub destroy: Option<DestroyFn>,
    pub render_cpu: Option<RenderCpuFn>,
}

#[repr(C)]
pub struct VePluginDesc {
    pub struct_size: u32,
    pub abi_version: u32,
    pub name: *const c_char,
    pub vendor: *const c_char,
    pub effects: *const *const VeEffectDesc,
    pub effect_count: u32,
}

/// Smallest sizes the host accepts: the layouts of ABI version 1. Frozen —
/// later versions only append, and fields a smaller (older) struct lacks read
/// as zero/null, which every appended field must treat as "not provided".
pub const VE_PLUGIN_DESC_MIN_SIZE: usize = std::mem::size_of::<VePluginDesc>();
pub const VE_EFFECT_DESC_MIN_SIZE: usize = std::mem::size_of::<VeEffectDesc>();
pub const VE_PARAM_DESC_MIN_SIZE: usize = std::mem::size_of::<VeParamDesc>();

pub type VePluginEntryFn = unsafe extern "C" fn(*const VeHost) -> *const VePluginDesc;

/// The entry symbol: `ve_plugin_entry`, or `ve_plugin_entry_<name>` for a
/// statically linked plugin.
pub fn entry_symbol(static_name: Option<&str>) -> String {
    match static_name {
        Some(n) => format!("ve_plugin_entry_{n}"),
        None => "ve_plugin_entry".into(),
    }
}
