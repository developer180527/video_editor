/*
 * Invert: the smallest complete ve plugin. One filter with one parameter,
 * both a GPU (WGSL) and a CPU implementation.
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

static int32_t render_cpu(void *instance, const VeRenderArgs *args, const VeImage *const *inputs, uint32_t input_count,
                          VeImage *out) {
    (void)instance;
    if (input_count != 1) return -1;
    const VeImage *in = inputs[0];
    double amount = args->params.count > 0 ? args->params.values[0][0] : 1.0;
    for (uint32_t y = 0; y < out->height; y++) {
        const float *src = (const float *)((const char *)in->pixels + y * in->stride);
        float *dst = (float *)((char *)out->pixels + y * out->stride);
        for (uint32_t x = 0; x < out->width; x++) {
            float a = src[4 * x + 3];
            for (int c = 0; c < 3; c++) {
                float v = src[4 * x + c];
                dst[4 * x + c] = (float)(v + ((a - v) - v) * amount);
            }
            dst[4 * x + 3] = a;
        }
    }
    return 0;
}

static const VeEffectDesc invert = {
    .struct_size = sizeof(VeEffectDesc),
    .kind = VE_KIND_FILTER,
    .id = "org.ve.examples.invert",
    .name = "Invert",
    .category = "Color",
    .major_version = 1,
    .minor_version = 0,
    .flags = VE_FLAG_THREAD_SAFE,
    .params = params,
    .param_count = 1,
    .wgsl = wgsl,
    .render_cpu = render_cpu,
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
