# libgui, as it matters to this video editor

Sources: `~/Developer/libgui/README.md`, `MANUAL.md`, and the demo
`crates/demo_gui/libgui_cut` (a copy lives in `./libgui_cut`, identical apart
from the missing licence symlinks).

## What libgui is

A Rust **UI core only**: immediate-mode API, retained per-`Id` state, layout
solved after the frame is built. Input in (`InputEvent`), draw data out
(`FrameOutput`: 96-byte instances + batches + glyph atlas + `PlatformOutput`).
It owns **no window, GPU, clock, threads, filesystem**. C ABI (`libgui.h`) and
C++ header (`libgui.hpp`) via `crates/libgui_c`. Pre-1.0: pin a version.

Frame loop: `push(events)` → `begin_frame(FrameInfo{size, scale, dt})` →
widgets → `end_frame()` → renderer draws batches → apply platform output.
Idle: `repaint_after == None` means sleep; `needs_frame_for(&info, idle)`
gates rebuilding; skipped frames re-draw old batches (`render_batches`).

## Pieces the editor will lean on

| Need | libgui feature |
|---|---|
| Panel layout (Premiere-style) | `DockState<Tab>` + `TabViewer`; tear-off windows; layouts saved as TOML; tab ids via `Id::from_name` |
| Timeline | Custom leaf: `add_leaf` + `Painter` + `interact_drag`; or `canvas` for pan/zoom in model coords |
| Video in the Program/Source monitor | `ui.viewport(texture)` — size the target from `ui.rect_of(id)` **after** `end_frame` |
| Thumbnails / filmstrips / waveforms | `p.image*` with app-registered textures; `ui.cached` for waveform sections |
| Effect Controls | `tree_row`, `drag_value`, `validated_input`, `libgui_units::number_input`, `color_button` (`finished` = push undo) |
| Project bin | `virtual_list` / `table`, `selectable` + `select()` for multi-select, `drag_source`/`drop_zone` to timeline, `begin/end_external_drag` for OS file drops |
| Menus, shortcuts | `menu_button`, `context_menu`, `consume_shortcut`, `libgui_keymap` |
| Notifications | `toast` + `show_toasts` (export done/failed) |
| Motion | `animate_bool`, springs; respect `reduced_motion` |
| Tests | headless `Ui`, `libgui_soft` golden images, `Budget` frame-cost asserts |

Rendering: `libgui_wgpu::Renderer` (prepare/render). Own renderer needs one
pipeline from `libgui_shaders` (WGSL/HLSL/MSL/GLSL/SPIR-V), premultiplied
alpha; `libgui::render_contract` documents the bytes.

## Gaps that touch an NLE (as of da04dc8, 2026-10-08, vendored in third_party/libgui)

Fixed upstream: `meter`/`meter_with_average` (`MeterOptions::audio_db()`),
`scope` traces, `dashed_line`/`dashed_polyline`, `image_rotated`/`text_rotated`,
`ui.modal`, paged glyph atlas + triangle primitive (render contract 5),
`text_area` word wrap (on by default), right-to-left text (`bidi` feature).

Still open:
- Accessibility (screen readers); panels not mirrored for RTL languages.
- Scopes have no axes/trigger/zoom → video scopes are our own GPU textures.
- Rotation is draw-only (no rotated hit-testing).

Upgrade notes: read `third_party/libgui/CHANGELOG.md` when moving the pin.
Contract 5 matters only for a hand-ported shader; we use `libgui_wgpu`.

## The `libgui_cut` demo (the screenshot)

~3.2k lines, **UI only — no media**. winit + wgpu host, one OS window per
dock surface sharing one device.

| File | Role |
|---|---|
| `main.rs` | winit/wgpu host: multi-window dock sync, swapchain resize per drawn frame, `needs_frame_for` gating, ticks `Editor` clock |
| `lib.rs` | `App { ed: Editor, dock }`, `ui_for(surface)` |
| `state.rs` | `Editor`, `Sequence`, `Track`, `Clip`, `Asset` — hard-coded sample data; times are `f32` seconds; `timecode()` assumes 24 fps |
| `dock.rs` | `Tab` enum, stable keys, initial 2×2 layout |
| `timeline.rs` | one custom surface: ruler, headers, 2-axis scroll, zoom about pointer, clip drag between tracks, snapping, tools column, meters |
| `program.rs` | monitor: picture is **painted rectangles** (placeholder for `ui.viewport`), scrub bar, transport |
| `effects.rs` | Effect Controls tree with keyframe lane |
| `project.rs` | bin thumbnails |
| `topbar.rs`, `widgets.rs`, `theme.rs` | chrome, icon paths, palette |

### What must change to become a real editor

1. **Model**: `f32` seconds → rational time / frame counts; clips need
   `asset_id`, `src_in/out`, effect stacks; edits go through commands (undo).
   `Editor` mixes document and view state (`pps`, `scroll_*`, `drag`) — split
   them.
2. **Monitor**: replace `program::frame` painting with `ui.viewport` showing
   the compositor's wgpu texture (same device as the UI).
3. **Clock**: `Editor::tick(dt)` advances the playhead from frame `dt`; the
   playback engine (audio clock) must own the playhead instead.
4. **Effect Controls**: driven by plugin parameter descriptors, not fixed fields.
5. **Bin**: real assets, thumbnails decoded by FFmpeg, uploaded as textures.

### Stack implication

libgui's native path is **Rust + wgpu + winit**. Using SDL3 is possible (write
an SDL→`InputEvent` translator modelled on `libgui_winit`, ~200 lines), but
then the UI renderer must be ported to SDL_GPU. Recommendation: keep
winit + wgpu for UI *and* compositing (one device, zero-copy into
`ui.viewport`), use FFmpeg (`ffmpeg-next`/`rsmpeg`) for media and
`cpal` (or SDL3 audio only) for output. Plugins stay a C ABI either way.
