// Compiles the C layout probe used by tests/layout.rs.
fn main() {
    println!("cargo:rerun-if-changed=include/ve_plugin.h");
    println!("cargo:rerun-if-changed=tests/layout.c");
    // C11 for _Static_assert (MSVC needs /std:c11 to accept it).
    cc::Build::new().file("tests/layout.c").include("include").std("c11").compile("ve_abi_layout");
}
