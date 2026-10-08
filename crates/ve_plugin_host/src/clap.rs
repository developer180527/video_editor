//! CLAP audio plugin host (desktop). Headers: third_party/clap/include.
//! Scheduled with the audio engine; this module only lists installed plugins.

use ve_ports::NativeLibraries;

pub fn discover(libs: &dyn NativeLibraries) -> Vec<String> {
    libs.discover("clap")
}
