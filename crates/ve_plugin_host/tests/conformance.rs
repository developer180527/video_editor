//! ABI v1 conformance: each rule in sdk/ABI.md, as a plugin built by hand
//! the way a C plugin lays out its static data, loaded through the real
//! loader. If the host stops keeping a promise, one of these fails.

use std::cell::Cell;
use std::ffi::{c_void, CStr};
use std::mem::size_of;
use std::sync::Arc;

use ve_plugin_abi::*;
use ve_plugin_host::{native, EffectInfo, LinkedLibrary, PluginError};

const GOOD: &CStr = c"@fragment fn effect(i: EffectIn) -> @location(0) vec4<f32> { return textureSample(source, source_sampler, i.uv); }";

thread_local! {
    /// The descriptor the entry point returns, for the plugin being loaded
    /// on this thread (the loader calls the entry on the caller's thread).
    static DESC: Cell<*const VePluginDesc> = const { Cell::new(std::ptr::null()) };
}

unsafe extern "C" fn entry(_host: *const VeHost) -> *const VePluginDesc {
    DESC.with(|d| d.get())
}

fn param(ty: u32) -> VeParamDesc {
    VeParamDesc {
        struct_size: size_of::<VeParamDesc>() as u32,
        ty: VeParamType(ty),
        id: c"amount".as_ptr(),
        label: c"Amount".as_ptr(),
        min: 0.0,
        max: 1.0,
        default_value: [0.5, 0.0, 0.0, 0.0],
        choices: std::ptr::null(),
        choice_count: 0,
        flags: VE_PARAM_ANIMATABLE,
    }
}

fn effect(id: &'static CStr, params: &'static [VeParamDesc]) -> VeEffectDesc {
    VeEffectDesc {
        struct_size: size_of::<VeEffectDesc>() as u32,
        kind: VeEffectKind::Filter,
        id: id.as_ptr(),
        name: c"Test".as_ptr(),
        category: c"Tests".as_ptr(),
        major_version: 1,
        minor_version: 0,
        flags: 0,
        params: params.as_ptr(),
        param_count: params.len() as u32,
        wgsl: GOOD.as_ptr(),
        create: None,
        destroy: None,
        render_cpu: None,
    }
}

/// Load a plugin made of `effects` (leaked, as static data is), after
/// `tweak` has had its way with the descriptor.
fn load(effects: Vec<VeEffectDesc>, tweak: impl FnOnce(&mut VePluginDesc)) -> Result<Vec<EffectInfo>, PluginError> {
    let effects: &'static [VeEffectDesc] = Box::leak(effects.into_boxed_slice());
    let ptrs: &'static [*const VeEffectDesc] = Box::leak(effects.iter().map(|e| e as *const _).collect::<Vec<_>>().into_boxed_slice());
    let mut desc = VePluginDesc {
        struct_size: size_of::<VePluginDesc>() as u32,
        abi_version: VE_ABI_VERSION,
        name: c"conformance".as_ptr(),
        vendor: c"tests".as_ptr(),
        effects: ptrs.as_ptr(),
        effect_count: ptrs.len() as u32,
    };
    tweak(&mut desc);
    let desc: &'static VePluginDesc = Box::leak(Box::new(desc));
    DESC.with(|d| d.set(desc));
    let lib = LinkedLibrary::new("conformance").with("ve_plugin_entry_conformance", entry as *const c_void);
    native::load(Arc::new(lib), Some("conformance"))
}

fn params(ty: u32) -> &'static [VeParamDesc] {
    Box::leak(Box::new([param(ty)]))
}

fn refused(r: Result<Vec<EffectInfo>, PluginError>, why: &str) {
    match r {
        Ok(_) => panic!("loaded, but should be refused: {why}"),
        Err(e) => assert!(e.to_string().contains(why), "refused, but not for `{why}`: {e}"),
    }
}

#[test]
fn a_well_formed_v1_plugin_loads() {
    let fx = load(vec![effect(c"org.test.ok", params(VeParamType::Float.0))], |_| {}).unwrap();
    assert_eq!(fx[0].plugin.id, "org.test.ok");
    assert_eq!(fx[0].params[0].default[0], 0.5);
    assert!(fx[0].params[0].animatable);
}

/// Rule: structs grow by appending, so the smallest struct a host accepts
/// is the v1 one, forever: a v1 plugin loads in every later host, which
/// reads the fields it appended as absent (zero) — `read_versioned`'s unit
/// tests cover that reading. Pinned here: the floor is exactly v1.
#[test]
fn the_v1_structs_are_the_floor() {
    assert_eq!((VE_PLUGIN_DESC_MIN_SIZE, VE_EFFECT_DESC_MIN_SIZE, VE_PARAM_DESC_MIN_SIZE), (40, 96, 88));
    // Exactly v1, with the reserved CPU fields set: loads as a shader effect.
    let mut e = effect(c"org.test.v1", params(VeParamType::Float.0));
    e.render_cpu = Some(dummy_render);
    assert!(load(vec![e], |_| {}).is_ok());
}

unsafe extern "C" fn dummy_render(_: *mut c_void, _: *const VeRenderArgs, _: *const *const VeImage, _: u32, _: *mut VeImage) -> i32 {
    unreachable!("v1 hosts never call render_cpu")
}

/// Rule: a struct smaller than its v1 size is not a v1 struct.
#[test]
fn structs_below_v1_are_refused() {
    let mut e = effect(c"org.test.tiny", params(VeParamType::Float.0));
    e.struct_size = 16;
    refused(load(vec![e], |_| {}), "VeEffectDesc too small");
    refused(load(vec![], |d| d.struct_size = 8), "VePluginDesc too small");
}

/// Rule: a plugin built for a newer ABI than the host speaks is refused,
/// as is one claiming version 0.
#[test]
fn abi_versions_are_checked() {
    refused(load(vec![], |d| d.abi_version = VE_ABI_VERSION + 1), "built for ABI 2");
    refused(load(vec![], |d| d.abi_version = 0), "built for ABI 0");
}

/// Rule: enum values a host does not know are refused, never guessed at.
#[test]
fn unknown_kinds_and_types_are_refused() {
    let mut e = effect(c"org.test.kind", &[]);
    e.kind = VeEffectKind(3);
    refused(load(vec![e], |_| {}), "unknown effect kind 3");
    refused(load(vec![effect(c"org.test.type", params(6))], |_| {}), "type this host does not know");
}

/// Rule: every v1 effect has WGSL; the CPU path is reserved and an effect
/// with only that is refused (it would load and render nothing).
#[test]
fn shaders_are_required_and_checked() {
    let mut cpu_only = effect(c"org.test.cpu", &[]);
    cpu_only.wgsl = std::ptr::null();
    cpu_only.render_cpu = Some(dummy_render);
    refused(load(vec![cpu_only], |_| {}), "has no WGSL");
    let mut broken = effect(c"org.test.broken", &[]);
    broken.wgsl = c"@fragment fn effect(i: EffectIn) -> @location(0) vec4<f32> { return nope; }".as_ptr();
    refused(load(vec![broken], |_| {}), "does not meet the WGSL contract");
    // With a CPU path as well as WGSL: loads, and the CPU path is never used.
    let mut both = effect(c"org.test.both", &[]);
    both.render_cpu = Some(dummy_render);
    assert!(load(vec![both], |_| {}).is_ok());
}

/// Rule: an effect is identified by id and major version, once per plugin.
#[test]
fn duplicate_effects_are_refused() {
    let one = effect(c"org.test.dup", &[]);
    let two = effect(c"org.test.dup", &[]);
    refused(load(vec![one, two], |_| {}), "declared twice");
    let mut v2 = effect(c"org.test.dup", &[]);
    v2.major_version = 2;
    assert!(load(vec![effect(c"org.test.dup", &[]), v2], |_| {}).is_ok(), "a new major version is a new effect");
}

/// Rule: a plugin may decline to load (its entry returns NULL).
#[test]
fn a_plugin_may_decline() {
    DESC.with(|d| d.set(std::ptr::null()));
    let lib = LinkedLibrary::new("conformance").with("ve_plugin_entry_conformance", entry as *const c_void);
    assert!(matches!(native::load(Arc::new(lib), Some("conformance")), Err(PluginError::Declined)));
}
