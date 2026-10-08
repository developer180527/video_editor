use std::mem::{offset_of, size_of};
use ve_plugin_abi::*;

extern "C" {
    fn ve_sizeof_host() -> usize;
    fn ve_sizeof_param_desc() -> usize;
    fn ve_sizeof_param_values() -> usize;
    fn ve_sizeof_image() -> usize;
    fn ve_sizeof_render_args() -> usize;
    fn ve_sizeof_effect_desc() -> usize;
    fn ve_sizeof_plugin_desc() -> usize;
    fn ve_offsetof_effect_wgsl() -> usize;
    fn ve_offsetof_args_params() -> usize;
}

#[test]
fn rust_mirror_matches_c_layout() {
    unsafe {
        assert_eq!(size_of::<VeHost>(), ve_sizeof_host());
        assert_eq!(size_of::<VeParamDesc>(), ve_sizeof_param_desc());
        assert_eq!(size_of::<VeParamValues>(), ve_sizeof_param_values());
        assert_eq!(size_of::<VeImage>(), ve_sizeof_image());
        assert_eq!(size_of::<VeRenderArgs>(), ve_sizeof_render_args());
        assert_eq!(size_of::<VeEffectDesc>(), ve_sizeof_effect_desc());
        assert_eq!(size_of::<VePluginDesc>(), ve_sizeof_plugin_desc());
        assert_eq!(offset_of!(VeEffectDesc, wgsl), ve_offsetof_effect_wgsl());
        assert_eq!(offset_of!(VeRenderArgs, params), ve_offsetof_args_params());
    }
}
