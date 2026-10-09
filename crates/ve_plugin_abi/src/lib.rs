//! Rust mirror of `include/ve_plugin.h`. The header is the source of truth;
//! `tests/layout.rs` checks every struct against the C compiler's layout.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_void};

pub const VE_ABI_VERSION: u32 = 1;
pub const VE_PARAM_ANIMATABLE: u32 = 1 << 0;
pub const VE_FLAG_THREAD_SAFE: u32 = 1 << 0;

/// The header, for tools that ship it (the SDK, the CLI's `sdk` command).
pub const HEADER: &str = include_str!("../include/ve_plugin.h");

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VeLogLevel {
    Debug = 0,
    Info = 1,
    Warn = 2,
    Error = 3,
}

#[repr(C)]
pub struct VeHost {
    pub struct_size: u32,
    pub abi_version: u32,
    pub log: Option<unsafe extern "C" fn(VeLogLevel, *const c_char)>,
    pub get_extension: Option<unsafe extern "C" fn(*const c_char) -> *const c_void>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VeParamType {
    Bool = 0,
    Int = 1,
    Float = 2,
    Vec2 = 3,
    Color = 4,
    Choice = 5,
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

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VeEffectKind {
    Filter = 0,
    Transition = 1,
    Generator = 2,
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
