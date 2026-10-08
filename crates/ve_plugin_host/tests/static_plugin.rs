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
