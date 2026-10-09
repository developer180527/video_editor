//! Loads the SDK example the way iPadOS loads every plugin: linked in, found
//! through a `LinkedLibrary`, never `dlopen`ed.

use std::sync::Arc;
use ve_model::PluginApi;
use ve_plugin_host::native::{self, CpuImage};
use ve_plugin_host::{EffectKind, Implementation, LinkedLibrary, Registry};


#[test]
fn load_and_render_linked_plugin() {
    let lib = ve_builtins::linked()[0]();
    let name = lib.name().to_string();
    let effects = native::load(Arc::from(lib), Some(&name)).expect("loads");
    let mut reg = Registry::default();
    effects.into_iter().for_each(|e| reg.add(e));

    let fx = reg.find_id(PluginApi::Native, "org.ve.examples.invert").expect("registered");
    assert_eq!(fx.kind, EffectKind::Filter);
    assert_eq!(fx.params[0].id, "amount");
    assert!(fx.params[0].animatable);
    assert!(fx.wgsl.as_deref().unwrap().contains("@fragment"));

    let Implementation::Native(n) = &fx.implementation else { panic!("has a CPU path") };
    let mut input = CpuImage::new(2, 1);
    input.pixels.copy_from_slice(&[0.25, 0.5, 1.0, 1.0, 0.0, 0.0, 0.0, 0.5]);
    let mut out = CpuImage::new(2, 1);
    n.render_cpu(0.0, &[[1.0, 0.0, 0.0, 0.0]], &mut [&mut input], &mut out).unwrap();
    // Premultiplied invert: a - c.
    assert_eq!(out.pixels, [0.75, 0.5, 0.0, 1.0, 0.5, 0.5, 0.5, 0.5]);
}

#[test]
fn missing_entry_is_an_error() {
    let lib = LinkedLibrary::new("nothing");
    assert!(native::load(Arc::new(lib), Some("nothing")).is_err());
}

/// A plugin built against a newer header, whose structs carry fields this
/// host does not know: they are skipped, and the parameter array is walked
/// at the plugin's stride, not ours.
#[test]
fn newer_plugins_with_larger_structs_load() {
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use ve_plugin_abi::*;

    #[repr(C)]
    struct NewerParam {
        base: VeParamDesc,
        extra: [u64; 3],
    }
    #[repr(C)]
    struct NewerEffect {
        base: VeEffectDesc,
        extra: u64,
    }

    fn param(id: &'static std::ffi::CStr, default: f64) -> NewerParam {
        NewerParam {
            base: VeParamDesc {
                struct_size: std::mem::size_of::<NewerParam>() as u32,
                ty: VeParamType::Float,
                id: id.as_ptr(),
                label: std::ptr::null(),
                min: 0.0,
                max: 1.0,
                default_value: [default, 0.0, 0.0, 0.0],
                choices: std::ptr::null(),
                choice_count: 0,
                flags: 0,
            },
            extra: [u64::MAX; 3],
        }
    }

    // Built once and leaked, like a plugin's static data (as addresses: raw
    // pointers are not Sync).
    static DESC: OnceLock<usize> = OnceLock::new();
    unsafe extern "C" fn entry(_host: *const VeHost) -> *const VePluginDesc {
        *DESC.get_or_init(|| {
            let params: &'static [NewerParam] = Box::leak(Box::new([param(c"first", 0.25), param(c"second", 0.75)]));
            let effect: &'static NewerEffect = Box::leak(Box::new(NewerEffect {
                base: VeEffectDesc {
                    struct_size: std::mem::size_of::<NewerEffect>() as u32,
                    kind: VeEffectKind::Filter,
                    id: c"org.test.newer".as_ptr(),
                    name: std::ptr::null(),
                    category: std::ptr::null(),
                    major_version: 1,
                    minor_version: 0,
                    flags: 0,
                    params: params.as_ptr() as *const VeParamDesc,
                    param_count: 2,
                    wgsl: c"@fragment fn effect() {}".as_ptr(),
                    create: None,
                    destroy: None,
                    render_cpu: None,
                },
                extra: 42,
            }));
            let effects: &'static [*const VeEffectDesc; 1] = Box::leak(Box::new([effect as *const NewerEffect as *const VeEffectDesc]));
            let desc = Box::leak(Box::new(VePluginDesc {
                struct_size: std::mem::size_of::<VePluginDesc>() as u32 + 16, // and more fields after
                abi_version: VE_ABI_VERSION,
                name: std::ptr::null(),
                vendor: std::ptr::null(),
                effects: effects.as_ptr(),
                effect_count: 1,
            }));
            desc as *const VePluginDesc as usize
        }) as *const VePluginDesc
    }

    let lib = LinkedLibrary::new("newer").with("ve_plugin_entry_newer", entry as *const c_void);
    let fx = native::load(Arc::new(lib), Some("newer")).expect("loads").remove(0);
    let got: Vec<(String, f64)> = fx.params.iter().map(|p| (p.id.clone(), p.default[0])).collect();
    assert_eq!(got, [("first".to_string(), 0.25), ("second".to_string(), 0.75)]);
}
