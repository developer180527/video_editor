/*
 * Invert: the smallest complete ve plugin (ABI v1). One filter with one
 * animatable parameter, rendered by its WGSL shader on the GPU. Pixels are
 * premultiplied, so "white minus colour" is alpha minus colour.
 *
 *   dynamic:  cc -shared -fPIC -I../../../crates/ve_plugin_abi/include invert.c -o invert.vep
 *   static:   cc -c -DVE_PLUGIN_STATIC_NAME=invert ...   (iPadOS, built-ins)
 */
#include "ve_plugin.h"

static const VeParamDesc params[] = {
    {
        .struct_size = sizeof(VeParamDesc),
        .type = VE_PARAM_FLOAT,
        .id = "amount",
        .label = "Amount",
        .min = 0.0,
        .max = 1.0,
        .default_value = {1.0, 0, 0, 0},
        .flags = VE_PARAM_ANIMATABLE,
    },
};

static const char wgsl[] =
    "@fragment fn effect(in: EffectIn) -> @location(0) vec4<f32> {\n"
    "    let c = textureSample(source, source_sampler, in.uv);\n"
    "    let amount = params.values[0].x;\n"
    "    return vec4<f32>(mix(c.rgb, vec3<f32>(c.a) - c.rgb, amount), c.a);\n"
    "}\n";

static const VeEffectDesc invert = {
    .struct_size = sizeof(VeEffectDesc),
    .kind = VE_KIND_FILTER,
    .id = "org.ve.examples.invert",
    .name = "Invert",
    .category = "Color",
    .major_version = 1,
    .minor_version = 0,
    .params = params,
    .param_count = 1,
    .wgsl = wgsl,
    /* create, destroy, render_cpu: reserved in v1, left NULL. */
};

static const VeEffectDesc *const effects[] = {&invert};

static const VePluginDesc plugin = {
    .struct_size = sizeof(VePluginDesc),
    .abi_version = VE_ABI_VERSION,
    .name = "Examples",
    .vendor = "ve",
    .effects = effects,
    .effect_count = 1,
};

VE_EXPORT const VePluginDesc *VE_PLUGIN_ENTRY(const VeHost *host) {
    if (!host || host->abi_version < 1) return 0;
    return &plugin;
}
