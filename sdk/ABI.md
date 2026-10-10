# Plugin ABI v1 — the stability promise

A plugin built against [`ve_plugin.h`](../crates/ve_plugin_abi/include/ve_plugin.h)
v1 loads and renders the same in every later version of the editor. This
page is what that promise covers, how the ABI may grow without breaking it,
and what a plugin must do in return. Each rule has a test in
`crates/ve_plugin_host/tests/conformance.rs` or `crates/ve_plugin_abi/tests/`.

## What is frozen

- **The structs**: `VeHost`, `VeParamDesc`, `VeParamValues`, `VeImage`,
  `VeRenderArgs`, `VeEffectDesc`, `VePluginDesc` — every field, its order,
  size and offset. (Pinned as numbers on 64-bit targets, in Rust and in C.)
- **The values**: `VE_ABI_VERSION` = 1; every `VeParamType`, `VeEffectKind`
  and `VeLogLevel` value; `VE_PARAM_ANIMATABLE`.
- **The entry points**: `ve_plugin_entry` (dynamic), and
  `ve_plugin_entry_<name>` (statically linked, `-DVE_PLUGIN_STATIC_NAME`).
- **The shader contract**: [`WGSL_CONTRACT.md`](WGSL_CONTRACT.md) — the
  prelude, the bindings, `effect`'s signature, pixel format, parameter
  packing, the colour rule, `time`, `progress` and `scale`.

## How v1 grows (without breaking anything)

- **Structs only grow at the end.** Every struct starts with `struct_size`;
  a reader copies what both sides know. Fields a newer struct appends are
  ignored by older hosts; fields an older plugin lacks read as zero — so an
  appended field's zero value always means "not provided".
- **The v1 size is the floor.** A struct smaller than its v1 size is not a
  v1 struct and is refused.
- **New enum values are refused by hosts that don't know them**, never
  guessed at: a plugin using a parameter type added later fails to load on
  an older host, with a message saying so.
- **New capabilities are extensions, asked for by name** through
  `VeHost.get_extension("ve.<name>.v<n>")`. A plugin that needs one checks
  for it and declines (`return NULL`) or degrades if it is missing.
- **A plugin built for a newer ABI** (`abi_version` > the host's) is
  refused; so is `abi_version` 0.

## What is reserved

- **The CPU path** — `create`, `destroy`, `render_cpu`. v1 hosts render on
  the GPU and never call them; leave them NULL. A later host may offer CPU
  rendering as an extension a plugin opts into.
- **`get_extension`** returns NULL for everything in v1.
- **Shader names starting `ve_`** belong to the host.

## What a plugin must do

- **Give every effect a WGSL shader** that meets the contract. An effect
  without one is refused (it would load and render nothing).
- **Keep ids stable.** An effect's `id` (reverse DNS) and each parameter's
  `id` are saved in projects. Never reuse an id for something else.
- **Never change what a parameter means.** Add a new parameter instead.
  Renaming its `label` is fine; its `id`, type and units are not.
- **A major version is a new effect.** Changing an effect incompatibly
  (parameters removed, a different look for the same values) means bumping
  `major_version`: projects keep the old one, new uses get the new one. One
  plugin may ship both. The same id and major version twice in one plugin is
  refused.
- **Static data lives as long as the plugin**: every pointer in the
  descriptors (strings, arrays) must stay valid after the entry returns.

## What the host guarantees

- Pixels are premultiplied, linear light, in the sequence's working space,
  in and out.
- Parameters arrive keyframed to the current frame, packed as the contract
  says; colours converted to linear working space.
- `source` and `source_b` carry full mip chains.
- `scale` tells you the texels per full-quality pixel; `time` and
  `progress` are as documented.
- A shader is checked when the plugin loads; a broken one is refused there,
  with its error, not on the GPU mid-edit.
