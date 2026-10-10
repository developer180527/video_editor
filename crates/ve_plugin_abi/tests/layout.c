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

/* ABI v1 is frozen: the same numbers tests/frozen.rs checks, here at
 * compile time, so the header itself cannot drift. */
#if UINTPTR_MAX == 0xffffffffffffffffu
#define VE_FROZEN(cond) _Static_assert(cond, #cond)
VE_FROZEN(VE_ABI_VERSION == 1);
VE_FROZEN(sizeof(VeHost) == 24 && offsetof(VeHost, log) == 8 && offsetof(VeHost, get_extension) == 16);
VE_FROZEN(sizeof(VeParamDesc) == 88 && offsetof(VeParamDesc, id) == 8 && offsetof(VeParamDesc, default_value) == 40 && offsetof(VeParamDesc, choices) == 72 && offsetof(VeParamDesc, flags) == 84);
VE_FROZEN(sizeof(VeParamValues) == 16 && offsetof(VeParamValues, values) == 8);
VE_FROZEN(sizeof(VeImage) == 32 && offsetof(VeImage, stride) == 16 && offsetof(VeImage, pixels) == 24);
VE_FROZEN(sizeof(VeRenderArgs) == 56 && offsetof(VeRenderArgs, params) == 32 && offsetof(VeRenderArgs, progress) == 48);
VE_FROZEN(sizeof(VeEffectDesc) == 96 && offsetof(VeEffectDesc, kind) == 4 && offsetof(VeEffectDesc, params) == 48 && offsetof(VeEffectDesc, wgsl) == 64 && offsetof(VeEffectDesc, render_cpu) == 88);
VE_FROZEN(sizeof(VePluginDesc) == 40 && offsetof(VePluginDesc, effects) == 24 && offsetof(VePluginDesc, effect_count) == 32);
VE_FROZEN(VE_PARAM_BOOL == 0 && VE_PARAM_INT == 1 && VE_PARAM_FLOAT == 2 && VE_PARAM_VEC2 == 3 && VE_PARAM_COLOR == 4 && VE_PARAM_CHOICE == 5);
VE_FROZEN(VE_KIND_FILTER == 0 && VE_KIND_TRANSITION == 1 && VE_KIND_GENERATOR == 2);
VE_FROZEN(VE_LOG_DEBUG == 0 && VE_LOG_ERROR == 3 && VE_PARAM_ANIMATABLE == 1u);
#endif
