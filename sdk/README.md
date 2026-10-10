# Plugin SDK

Native plugins are C (or anything exporting C) against the frozen v1 ABI
([what is promised](ABI.md)),
[`ve_plugin.h`](../crates/ve_plugin_abi/include/ve_plugin.h).

- **Example:** [`examples/invert/invert.c`](examples/invert/invert.c): one
  filter, one animatable parameter, a WGSL shader.
- **Desktop install:** build a shared library named `*.vep` and put it in the
  app's plugin folder (macOS: `~/Library/Application Support/VideoEditor/Plugins`).
- **iPadOS:** plugins cannot be loaded from files. They are compiled into the
  app with `-DVE_PLUGIN_STATIC_NAME=<name>` and listed in `crates/ve_builtins`.
- **Shaders:** every effect is a WGSL shader, per
  [`WGSL_CONTRACT.md`](WGSL_CONTRACT.md); it is checked when the plugin loads.
  The CPU fields are reserved in v1.

```bash
cc -shared -fPIC -I../crates/ve_plugin_abi/include examples/invert/invert.c -o invert.vep
```
