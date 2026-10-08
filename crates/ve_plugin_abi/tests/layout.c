/* Sizes of the ABI structs as the C compiler sees them; the Rust mirror is
 * checked against these in tests/layout.rs. */
#include "ve_plugin.h"
size_t ve_sizeof_host(void) { return sizeof(VeHost); }
size_t ve_sizeof_param_desc(void) { return sizeof(VeParamDesc); }
size_t ve_sizeof_param_values(void) { return sizeof(VeParamValues); }
size_t ve_sizeof_image(void) { return sizeof(VeImage); }
size_t ve_sizeof_render_args(void) { return sizeof(VeRenderArgs); }
size_t ve_sizeof_effect_desc(void) { return sizeof(VeEffectDesc); }
size_t ve_sizeof_plugin_desc(void) { return sizeof(VePluginDesc); }
size_t ve_offsetof_effect_wgsl(void) { return offsetof(VeEffectDesc, wgsl); }
size_t ve_offsetof_args_params(void) { return offsetof(VeRenderArgs, params); }
