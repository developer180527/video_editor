/*
 * ve_plugin.h — the native plugin ABI, version 1.
 *
 * A plugin is C (or anything that exports C): no C++ types, exceptions or
 * allocator crossings at this boundary. Every struct starts with
 * `struct_size`; a newer host or plugin appends fields and checks the size
 * before reading them, so old binaries keep working. Capabilities beyond v1
 * are asked for by name through VeHost.get_extension.
 *
 * The host owns parameters, keyframes, undo and UI. A plugin declares its
 * parameters and renders; it never draws interface.
 *
 * Two ways to render, so effects work everywhere:
 *   - `wgsl`: a WGSL fragment shader, run by the host on the GPU. Shader-only
 *     plugins carry no native code and run on every platform, iPad included.
 *   - `render_cpu`: a C function over RGBA float pixels.
 *
 * Loading: a dynamic plugin exports `ve_plugin_entry`. A statically linked
 * plugin (iPadOS, where loading code from files is forbidden) is built with
 * -DVE_PLUGIN_STATIC_NAME=<name> and exports `ve_plugin_entry_<name>`, so many
 * can be linked into one binary.
 */
#ifndef VE_PLUGIN_H
#define VE_PLUGIN_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define VE_ABI_VERSION 1u

#if defined(_WIN32)
#  define VE_EXPORT __declspec(dllexport)
#else
#  define VE_EXPORT __attribute__((visibility("default")))
#endif

#define VE_CAT_(a, b) a##b
#define VE_CAT(a, b) VE_CAT_(a, b)
#ifdef VE_PLUGIN_STATIC_NAME
#  define VE_PLUGIN_ENTRY VE_CAT(ve_plugin_entry_, VE_PLUGIN_STATIC_NAME)
#else
#  define VE_PLUGIN_ENTRY ve_plugin_entry
#endif

typedef enum VeLogLevel { VE_LOG_DEBUG = 0, VE_LOG_INFO = 1, VE_LOG_WARN = 2, VE_LOG_ERROR = 3 } VeLogLevel;

/* Services the host offers. Valid for the life of the process. */
typedef struct VeHost {
    uint32_t struct_size;
    uint32_t abi_version;
    void (*log)(VeLogLevel level, const char *message);
    /* A versioned extension table by name ("ve.gpu.v1"), or NULL. */
    const void *(*get_extension)(const char *name);
} VeHost;

typedef enum VeParamType {
    VE_PARAM_BOOL = 0,
    VE_PARAM_INT = 1,
    VE_PARAM_FLOAT = 2,
    VE_PARAM_VEC2 = 3,
    VE_PARAM_COLOR = 4, /* linear RGBA */
    VE_PARAM_CHOICE = 5,
} VeParamType;

typedef struct VeParamDesc {
    uint32_t struct_size;
    VeParamType type;
    const char *id;    /* stable: saved in projects */
    const char *label; /* shown to the user */
    double min, max;
    double default_value[4];
    /* VE_PARAM_CHOICE: `choice_count` labels. */
    const char *const *choices;
    uint32_t choice_count;
    uint32_t flags; /* VE_PARAM_ANIMATABLE, … */
} VeParamDesc;

#define VE_PARAM_ANIMATABLE (1u << 0)

/* Parameter values at one time, in declaration order, four doubles each. */
typedef struct VeParamValues {
    uint32_t count;
    const double (*values)[4];
} VeParamValues;

/* Linear-light RGBA, 32-bit float per channel, rows `stride` bytes apart. */
typedef struct VeImage {
    uint32_t struct_size;
    uint32_t width, height;
    size_t stride;
    float *pixels;
} VeImage;

typedef struct VeRenderArgs {
    uint32_t struct_size;
    /* Time from the clip's start, in seconds; and the frame duration. */
    double time, frame_duration;
    /* Render scale: 1.0 full resolution, 0.5 half-res preview. */
    double scale;
    VeParamValues params;
    /* Transitions: 0 → 1 across the transition. */
    double progress;
} VeRenderArgs;

typedef enum VeEffectKind {
    VE_KIND_FILTER = 0,     /* one input  */
    VE_KIND_TRANSITION = 1, /* two inputs */
    VE_KIND_GENERATOR = 2,  /* no input   */
} VeEffectKind;

#define VE_FLAG_THREAD_SAFE (1u << 0) /* render may run on several threads */

typedef struct VeEffectDesc {
    uint32_t struct_size;
    VeEffectKind kind;
    const char *id; /* reverse DNS: "com.example.invert" */
    const char *name;
    const char *category;
    uint32_t major_version, minor_version;
    uint32_t flags;
    const VeParamDesc *params;
    uint32_t param_count;

    /* GPU path (preferred): WGSL fragment shader source, or NULL. The host's
     * contract for it lives in sdk/WGSL_CONTRACT.md. */
    const char *wgsl;

    /* CPU path, or NULLs. `instance` is whatever `create` returned. */
    void *(*create)(const VeHost *host);
    void (*destroy)(void *instance);
    /* inputs: 0, 1 or 2 images by kind. Returns 0 on success. */
    int32_t (*render_cpu)(void *instance, const VeRenderArgs *args, const VeImage *const *inputs, uint32_t input_count,
                          VeImage *output);
} VeEffectDesc;

typedef struct VePluginDesc {
    uint32_t struct_size;
    uint32_t abi_version; /* VE_ABI_VERSION the plugin was built against */
    const char *name;
    const char *vendor;
    const VeEffectDesc *const *effects;
    uint32_t effect_count;
} VePluginDesc;

/* The one entry point. Returns NULL to decline (e.g. host too old). */
typedef const VePluginDesc *(*VePluginEntryFn)(const VeHost *host);

#ifdef __cplusplus
}
#endif
#endif /* VE_PLUGIN_H */
