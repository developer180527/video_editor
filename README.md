# Video Editor

A cross-platform (macOS, Windows, Linux, **iPadOS**) non-linear video editor.
Rust core, [libgui](third_party/libgui) UI, FFmpeg media, and a plugin system
spanning native C plugins, OpenFX, CLAP, sandboxed WASM add-ons and
out-of-process add-ons.

**Status: Phase C — real decode, compositing, playback with audio, export.** Next: Phase D, the plugin APIs. Read
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) first.

## Build

```bash
scripts/fetch_third_party.sh            # clone pinned sources into third_party/
scripts/build_ffmpeg.sh macos-arm64     # LGPL FFmpeg for this Mac
cargo test --workspace                  # everything, headless
scripts/check_arch.sh                   # dependency rules
cargo run -p editor_desktop --release   # the editor (Cmd+I or drop media on the Project panel)
cargo test -p ve_ui --test look         # renders the UI to target/ui-look/editor.png, no GPU needed
cargo run -p editor_cli -- probe clip.mov
scripts/ios_bundle.sh sim               # iPad simulator app → target/ios/sim/VideoEditor.app
scripts/macos_bundle.sh                 # macOS app → target/macos/Video Editor.app
cargo run -p editor_cli --release -- assemble demo.veproj a.mov b.mp4   # lay clips end to end
```

## Layout

| Path | |
|---|---|
| `crates/ve_*` | the engine: time, model, commands, ports, render, playback, plugins, engine API, UI |
| `crates/platform_*`, `media_ffmpeg`, `audio_cpal` | adapters: the only OS-specific code |
| `apps/` | composition roots: desktop, iPad, CLI |
| `sdk/` | plugin header docs and examples |
| `third_party/` | pinned sources (fetched, not committed) and their builds |
| `docs/` | architecture, libgui notes |
| `libgui_cut/` | the original UI mock-up, kept as the reference for Phase B |
