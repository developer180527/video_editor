// Built-in native plugins are compiled into the app and linked statically —
// the way every native plugin is linked on iPadOS — so built-ins exercise the
// same path a third-party plugin does on a device.
fn main() {
    println!("cargo:rerun-if-changed=../../sdk/examples/invert/invert.c");
    cc::Build::new()
        .file("../../sdk/examples/invert/invert.c")
        .include("../ve_plugin_abi/include")
        .define("VE_PLUGIN_STATIC_NAME", "invert")
        .compile("ve_example_invert");
}
