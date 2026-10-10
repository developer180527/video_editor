# WGSL effect contract — v1 (frozen)

Every effect is a WGSL fragment shader. This contract is part of plugin ABI
v1 ([`ABI.md`](ABI.md)) and is frozen: a v1 shader runs unchanged on every
later host.

## What the host puts before your source

Exactly this (it is `ve_plugin_abi::WGSL_PRELUDE`; a test keeps the two
identical):

```wgsl
struct EffectIn {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};
struct Params {
    values: array<vec4<f32>, 64>,
    time: f32,
    progress: f32,
    scale: f32,
    _pad: f32,
};
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var source: texture_2d<f32>;
@group(0) @binding(2) var source_sampler: sampler;
@group(0) @binding(3) var source_b: texture_2d<f32>;
```

## What your source must provide

```wgsl
@fragment
fn effect(i: EffectIn) -> @location(0) vec4<f32> { ... }
```

The host checks this when the plugin loads (with naga): the prelude plus
your source must parse and validate, `effect` must be a `@fragment` entry
point returning `@location(0) vec4<f32>`, and no entry point may be named
`ve_*` (the host's prefix). A shader that fails is refused at load with the
error and its line, counted from the start of your source.

## Pixels

- `source` (and `source_b` for transitions) and your output are
  **premultiplied, linear light, in the sequence's working space**
  (ACEScg or linear Rec.709). Sample with `source_sampler`; `i.uv` is
  0..1 over the output, (0, 0) at the top left.
- Filters read `source`; transitions read `source` (outgoing) and
  `source_b` (incoming); generators read neither.
- `source` and `source_b` carry a **full mip chain**, each level a 2×2
  average of the one above. Level 0 is the picture; a wide filter (a
  blur, a glow, a zoom) can read a pre-filtered level with
  `textureSampleLevel(source, source_sampler, uv, lod)` instead of
  thousands of taps. `textureSample` picks a level from the derivatives
  as usual (level 0 for a 1:1 read).
- Sampling outside 0..1 clamps to the edge pixels; to treat the outside
  as transparent, test the coordinate yourself.

## Parameters

`params.values[n]` is parameter `n` in declaration order, as four floats:

| Type    | Packing                                                       |
|---------|---------------------------------------------------------------|
| Bool    | `.x` 0 or 1                                                   |
| Int     | `.x`                                                          |
| Float   | `.x`                                                          |
| Vec2    | `.xy`                                                         |
| Choice  | `.x`, the index                                               |
| Color   | `.rgba`: **linear, working space, straight alpha** (see below) |

Values are at the current frame (keyframes already applied). At most 64
parameters reach the shader.

**Colour parameters** are stored as the user picked them (display-encoded,
what the colour picker and the monitor show). The host converts them for
the shader to linear light in the working space, straight (not
premultiplied) alpha — so a picked 50 % grey written to the output
appears on the monitor as 50 % grey, in either working space.

## Uniforms

- `params.time`: seconds since the clip's start.
- `params.progress`: transitions only, 0 → 1 across the transition.
- `params.scale`: pixels of the texture you read and write, per pixel of
  the picture at full quality. A filter runs on its clip's own picture: 1
  for a clip decoded whole (at any preview quality), 0.5 for a half-size
  proxy. A transition or generator draws the sequence frame: 0.5 in a
  half-size preview. Express sizes in the picture's pixels (a blur
  radius, an offset) and multiply by `scale` to get texels, so every
  quality looks like the full render.
