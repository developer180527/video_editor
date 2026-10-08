// Compiles the C layout probe used by tests/layout.rs.
fn main() {
    println!("cargo:rerun-if-changed=include/ve_plugin.h");
    println!("cargo:rerun-if-changed=tests/layout.c");
    cc::Build::new().file("tests/layout.c").include("include").compile("ve_abi_layout");
}
