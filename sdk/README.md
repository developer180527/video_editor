# Plugin SDK

Native plugins are C (or anything exporting C) against
[`ve_plugin.h`](../crates/ve_plugin_abi/include/ve_plugin.h).

- **Example:** [`examples/invert/invert.c`](examples/invert/invert.c): one
  filter, one animatable parameter, GPU (WGSL) and CPU paths.
- **Desktop install:** build a shared library named `*.vep` and put it in the
  app's plugin folder (macOS: `~/Library/Application Support/VideoEditor/Plugins`).
- **iPadOS:** plugins cannot be loaded from files. They are compiled into the
  app with `-DVE_PLUGIN_STATIC_NAME=<name>` and listed in `crates/ve_builtins`.
- **GPU path:** see [`WGSL_CONTRACT.md`](WGSL_CONTRACT.md). A shader-only
  plugin carries no native code and runs on every platform.

```bash
cc -shared -fPIC -I../crates/ve_plugin_abi/include examples/invert/invert.c -o invert.vep
```
