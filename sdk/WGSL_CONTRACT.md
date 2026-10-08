# WGSL effect contract (draft, frozen in Phase D)

A plugin's `wgsl` string is one fragment entry point named `effect`. The host
prepends these declarations:

```wgsl
struct EffectIn {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,        // 0..1 over the output
};

struct Params {
    values: array<vec4<f32>, 64>,      // declaration order; a scalar is .x
    time: f32,                          // seconds since the clip's start
    progress: f32,                      // transitions: 0 → 1
    scale: f32,                         // render scale (0.5 = half-res preview)
    _pad: f32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source: texture_2d<f32>;       // filters, transitions (A)
@group(0) @binding(2) var source_sampler: sampler;
@group(0) @binding(3) var source_b: texture_2d<f32>;     // transitions (B)
```

Pixels are **premultiplied, linear-light RGBA** in the sequence's working
space. Return the same.
