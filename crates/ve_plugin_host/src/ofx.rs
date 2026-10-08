//! OpenFX 1.5 image-effect host (desktop).
//!
//! Headers: third_party/openfx/include. OpenFX plugins are `.ofx.bundle`
//! directories holding a shared library that exports `OfxGetNumberOfPlugins`
//! and `OfxGetPlugin`. The host side is a set of "suites" (property, param,
//! image effect, memory, multithread, and the GPU suites) — a substantial
//! piece of work scheduled for Phase D. Until then this module only finds
//! installed bundles so the UI can list them.

use ve_ports::NativeLibraries;

/// Paths of installed `.ofx.bundle`s, from the platform's standard folders.
pub fn discover(libs: &dyn NativeLibraries) -> Vec<String> {
    libs.discover("ofx")
}
