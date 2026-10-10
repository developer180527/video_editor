# Plugin ABI v1 freeze — work orders

Goal: promise that a plugin built against `ve_plugin.h` v1 loads and
renders the same in every future host. Before that promise can be made,
the six problems found in review must be fixed. Each work order below is
self-contained: what is wrong, the decision, the change, and how we know
it is done. They run in three batches; each batch ends with the full
verification (tests, clippy `-D warnings`, `check_arch.sh`, iPad bundle).

**Status: all work orders done; ABI v1 is frozen.** WO-1, WO-2, WO-4
(batch A), WO-3 (batch B), WO-5, WO-6 (batch C) — each verified with the
full suite. The promise itself is [`sdk/ABI.md`](../sdk/ABI.md).

Done first: **WO-0** — enums read from plugin memory (`VeParamType`,
`VeEffectKind`, `VeLogLevel`) are `u32` newtypes; unknown values are
refused, not undefined behaviour (commit 8333720).

---

## Batch A — what v1 means

### WO-1  The CPU path is reserved, not half-supported
- **Problem.** An effect with only `render_cpu` loads (as
  `Implementation::Native`) but never renders: the compositor runs WGSL
  only. A plugin author would ship something that silently does nothing.
- **Decision.** v1 hosts render on the GPU. `wgsl` is required for every
  effect; `create`/`destroy`/`render_cpu` stay in the struct (the layout
  is frozen) but are *reserved*: v1 hosts never call them. A future host
  can offer CPU rendering through an extension (`ve.cpu.v1`), which a
  plugin asks for — the plugin, not the host, opts in.
- **Change.** Loader: an effect without `wgsl` is refused with a clear
  error; `render_cpu` on an effect with WGSL is ignored (logged once).
  Remove the unused CPU harness (`NativeEffect`, `render_cpu` host code)
  so nothing pretends to support it. Header comments say "reserved".
- **Done when.** A CPU-only effect is refused with a message naming it; an
  effect with both loads as shader-only; nothing in the host calls
  `render_cpu`.

### WO-2  What a colour parameter means
- **Problem.** The header says colour parameters are "linear RGBA", but
  the host passes shaders the stored value untouched, and what is stored
  is what the colour picker shows: sRGB-encoded (libgui's `Color`). The
  built-in generators make the opposite mistake: they treat the stored
  value as linear and encode it again, so a 50 % grey matte shows as ~75 %.
- **Decision.** One rule, everywhere: a colour parameter is **stored as
  the picker shows it — sRGB-encoded, straight alpha** (what a user
  types, what a project file holds). Each consumer converts once:
  - shader effects receive it **linear, in the sequence's working space,
    straight alpha** (the host decodes sRGB and applies the primaries);
  - the CPU generators draw it **as is** (their output is display-encoded).
- **Change.** `ve_render`: `pub fn display_color_to_working(c, space)`.
  `ve_engine::frame`: colour parameters go through it before upload.
  `ve_render::generate`: stop re-encoding colours. Header and contract
  state the rule.
- **Done when.** A GPU test: a plugin that outputs its colour parameter
  renders a picked 50 % grey as 50 % grey on the monitor (in both working
  spaces); a 50 % grey matte reads 128, not 188.

### WO-4  The layout is pinned, not just mirrored
- **Problem.** The layout test checks Rust against C. A change made to
  both (a field reordered in the header and the mirror) passes, and every
  shipped plugin breaks.
- **Decision.** v1 layouts are recorded as numbers: every struct's size
  and every field's offset on 64-bit targets.
- **Change.** A golden test with literal sizes and offsets (and the C side
  asserting the same via `_Static_assert` in `layout.c`), plus the enum
  values and `VE_ABI_VERSION == 1`.
- **Done when.** Moving or resizing any v1 field fails the build of the
  tests, on both the Rust and the C side.

## Batch B — catching broken shaders at load

### WO-3  WGSL is validated when the plugin loads
- **Problem.** A plugin with a typo in its shader loads fine and fails on
  the GPU at render time, as a toast mid-edit, possibly on every frame.
  Separately, the shader prelude (what the contract promises) is written
  twice: in `compositor.rs` and in `sdk/WGSL_CONTRACT.md`, free to drift.
- **Decision.** The prelude has one home: `ve_plugin_abi::WGSL_PRELUDE`,
  the frozen text. The compositor uses it; the contract document must
  contain it verbatim (a test checks). At load, the host validates
  `prelude + plugin wgsl` with naga (pure Rust, already in the build via
  wgpu): it must parse, validate, and have a `@fragment fn effect`
  returning `@location(0) vec4<f32>` with an `EffectIn` parameter.
- **Change.** `ve_plugin_host::wgsl::validate(src) -> Result<(), String>`;
  the loader refuses an effect that fails, with naga's message. Built-in
  shader effects (dissolve, dip, grade) pass through the same check in a
  test.
- **Done when.** Broken WGSL is refused at load with a line/column
  message; every built-in effect validates; the contract document and the
  compositor cannot disagree.

## Batch C — the promise, written and tested

### WO-5  The stability promise, in writing
- **Change.** `sdk/ABI.md`: what is frozen (structs, enum values,
  entry-point names, the WGSL prelude and bindings, parameter value
  packing, colour and alpha rules, time/progress/scale semantics); how it
  grows (append-only fields, zero means absent, new enum values refused by
  older hosts, extensions by name); what a plugin must do (stable effect
  and parameter ids; a major version bump is a new effect; never change a
  parameter's meaning); what the host guarantees (premultiplied linear
  working-space pixels, `scale`, `time`, `progress`); what is reserved
  (CPU path, extension names). Header and contract marked **frozen v1**.
- **Done when.** A plugin author can write a correct v1 plugin from the
  SDK alone.

### WO-6  Conformance tests
- **Change.** Loader tests with hand-built descriptors: shorter (older)
  structs load with new fields absent; a too-new `abi_version` is
  refused; unknown kind / parameter type refused; missing or broken WGSL
  refused; duplicate effect ids refused; the SDK example loads and
  renders (GPU test). Plus the WO-4 golden layout.
- **Done when.** Each rule in `sdk/ABI.md` has a test that fails if the
  host breaks it.
