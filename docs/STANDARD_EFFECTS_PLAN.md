# Standard effects — work orders

Goal: 40 effects and 25 transitions that ship with the app, written as
ABI v1 plugins (the same path a third-party plugin takes), so the frozen
ABI carries a full editor's worth of looks before anyone else uses it.

**Status: all work orders done.** Batch A (WO-1 `scale`, WO-2 mip
chains), batch B (WO-3 the plugin, WO-4 effects, WO-5 transitions), batch C
(WO-6 panel, WO-7 tests) — each verified with the full suite. A contact
sheet of all 65 renders with
`cargo test -p ve_builtins --test standard contact_sheet -- --ignored`
(→ `target/fx-look/sheet.ppm`).

## Batch A — what the host owes the effects

### WO-1  `scale` means one thing everywhere
- **Problem.** Effects run on the layer's own texture (a 1080p clip is a
  1920-wide texture even in a half-size preview; a proxy is smaller), but
  `params.scale` was output-per-sequence pixels. A 10 px blur covered 10
  texels at full quality and 5 in a half preview of the same texture.
- **Decision.** `scale` is *texture pixels per full-quality pixel of the
  picture being drawn*: 1 for a full-size clip at any preview quality, 0.5
  for a half-size proxy, out/sequence for transitions and CPU generators.
  Sizes a user types (a blur radius) are in the picture's own pixels.
- **Done when.** A GPU test: an effect sees scale 1 on a full clip in a
  half-size preview, and the contract says what the host does.

### WO-2  Wide filters can read a mip chain
- **Problem.** v1 effects are one pass. A wide blur (glow, drop shadow,
  blur dissolve) sampling level 0 either needs thousands of taps or shows
  ghost copies of edges.
- **Decision.** `source` and `source_b` come with a full mip chain (each
  level a 2×2 average of the one above). `textureSampleLevel(.., lod)`
  reads a pre-filtered level; level 0 is the picture, so every existing
  shader is unaffected. An additive guarantee, written into the contract.
- **Done when.** A GPU test reads a lower level of an effect's source and
  of both transition sources.

## Batch B — the pack

### WO-3  One plugin, built like a third party's
- `ve_builtins` gains `standard`: a Rust plugin that builds its
  `VePluginDesc` once and hands it out through `ve_plugin_entry_standard`;
  the host loads it with `native::load` like any plugin (naga validation
  included). Shaders live as `.wgsl` files with a shared `common.wgsl`
  prepended (colour helpers, sampling, noise) — readable examples for
  plugin authors.
- Ids are `org.ve.std.<name>`, version 1.0. Sizes are in picture pixels
  (× `scale`); points are percent of the frame (resolution-free);
  tone controls work on display-encoded values (50 % means 50 % on screen).

### WO-4  40 effects
| Category | Effects |
|---|---|
| Color Correction (12) | Brightness & Contrast, Levels, Hue/Saturation, Channel Mixer, Color Balance, Tint, Black & White, Photo Filter, Leave Color, Change to Color, Posterize, Threshold |
| Blur & Sharpen (5) | Gaussian Blur, Directional Blur, Radial Blur, Sharpen, Unsharp Mask |
| Distort (8) | Mirror, Corner Pin, Lens Distortion, Wave Warp, Twirl, Spherize, Ripple, Turbulent Displace |
| Transform (2) | Flip, Edge Feather |
| Stylize (8) | Glow, Mosaic, Emboss, Find Edges, Noise, Chromatic Aberration, Kaleidoscope, Vignette |
| Perspective (1) | Drop Shadow |
| Keying (3) | Chroma Key, Luma Key, Garbage Matte |
| Generators (1) | Gradient |

### WO-5  25 transitions
| Category | Transitions |
|---|---|
| Dissolve (6) | Dip to White, Additive Dissolve, Non-Additive Dissolve, Luma Fade, Blur Dissolve, Flash |
| Wipe (8) | Wipe, Barn Doors, Clock Wipe, Venetian Blinds, Checker Wipe, Random Blocks, Band Wipe, Inset |
| Iris (4) | Iris Round, Iris Box, Iris Diamond, Iris Cross |
| Slide (4) | Push, Slide, Split, Whip |
| Zoom & 3D (3) | Cross Zoom, Flip Over, Pixelate |

Cross Dissolve and Dip to Black move into "Dissolve".

## Batch C — using them

### WO-6  The Effects panel scales to 65 entries
- A search field; video transitions grouped by category like effects.
- A transition's parameters are editable in its panel (direction,
  softness, border, colour), one undo step each.

### WO-7  Tests
- The pack loads through the real loader; every shader validates (naga).
- On a GPU: every effect and transition compiles and renders at defaults
  without an error; every transition shows exactly A at 0 and B at 1;
  colour corrections at their neutral defaults leave a picture unchanged;
  blurs and sharpens leave a flat picture flat.

## Known limits (next ABI extensions, not v1)
- One pass per effect: true separable blurs, feedback and temporal
  effects need `ve.passes.v1`.
- No extra images: LUTs, gradient-wipe maps and track mattes need
  `ve.textures.v1`.
- Shaders don't know the working space: luma uses Rec.709 weights in
  both (close in ACEScg; exact needs `ve.colorspace.v1`).
