//! Plugins that ship inside the app.
//!
//! They are written against the public ABI (`ve_plugin.h`), exactly as a
//! third-party plugin is, so the ABI is exercised by our own effects first.
//! Each is compiled with `VE_PLUGIN_STATIC_NAME` and listed here as a
//! [`LinkedLibrary`] the platform hands to the engine.

use ve_plugin_abi::{VeHost, VePluginDesc};
use ve_plugin_host::LinkedLibrary;
use ve_ports::NativeLibrary;

extern "C" {
    fn ve_plugin_entry_invert(host: *const VeHost) -> *const VePluginDesc;
}

/// Every built-in plugin, as the platform's `NativeLibraries::linked` wants
/// them. A library's name is its static link name.
pub fn linked() -> Vec<fn() -> Box<dyn NativeLibrary>> {
    vec![|| Box::new(LinkedLibrary::new("invert").with("ve_plugin_entry_invert", ve_plugin_entry_invert as *const _))]
}
