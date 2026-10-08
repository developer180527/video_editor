# Architecture

This is the part of the editor that is expensive to change later. Features are
built on top of it; they should not need to change it. Each decision below is
recorded with its reason, so a future change is made knowingly.

## The shape

```
 apps/editor_desktop  apps/editor_ios  apps/editor_cli      composition roots (~40 lines each)
        │                   │                │
        ▼                   ▼                ▼
 ┌─────────────── ve_ui (libgui panels) ───────────────┐   platform_winit  ← the UI shell:
 │ reads Snapshots · sends Commands · drains Events     │   window, input, clipboard,
 └──────────────────────────┬───────────────────────────┘   wgpu surface (desktop + iPad)
                            ▼
 ┌──────────────────── ve_engine ───────────────────────┐
 │  the one public door: commands in; snapshots,        │
 │  frame plans and events out                          │
 ├──────────┬──────────┬───────────┬──────────┬─────────┤
 │ve_command│ve_render │ve_playback│ve_plugin_│ve_model │
 │undo/redo │evaluate +│transport, │host      │ve_time  │
 │          │compositor│audio clock│          │         │
 └──────────┴──────────┴───────────┴──────────┴─────────┘
                            │ uses only traits from
                            ▼
                        ve_ports   (System, Storage, MediaBackend, AudioOutput,
                            ▲       NativeLibraries, ProcessHost)
          implemented by    │
 platform_desktop · platform_ios · platform_headless · media_ffmpeg · audio_cpal
```

**Dependency rule:** arrows point down only. `scripts/check_arch.sh` enforces
it: core crates have no OS `cfg`s and never depend on adapters, the shell, the
UI or libgui; the UI talks to `ve_engine`, not to its internals.

## Crates

| Crate | Role | Depends on |
|---|---|---|
| `ve_time` | Exact time: ticks, rates, ranges, timecode | — |
| `ve_model` | The document; immutable, structurally shared | time |
| `ve_command` | Every edit as data, with its inverse; undo history | model |
| `ve_ports` | Traits for everything outside the engine | model |
| `ve_render` | `evaluate()` (what is visible) + wgpu `Compositor` | model, ports, wgpu |
| `ve_playback` | Transport; playhead read from the audio clock | time |
| `ve_media` | Background video decode + frame cache, audio mixer, real-time playback, thumbnails & waveform peaks | model, ports |
| `ve_plugin_abi` | `ve_plugin.h` + Rust mirror, layout-checked | — |
| `ve_plugin_host` | Loads plugins from any API into one `Registry` | abi, ports |
| `ve_engine` | The API frontends use | all of the above |
| `ve_builtins` | Built-in plugins, linked through the public ABI | abi, plugin_host |
| `ve_ui` | libgui panels (Premiere-style, ported from `libgui_cut`) | engine, render, libgui |
| `editor_app` | Glue: UI ↔ shell (dialogs, dock windows, waker); shared by desktop and iPad apps | ve_ui, platform_winit |
| `media_ffmpeg` | `MediaBackend` on FFmpeg 9 (LGPL) | ports |
| `audio_cpal` | `AudioOutput` on cpal | ports |
| `platform_headless` | std-only ports: tests, CLI; `FileStorage` | ports |
| `platform_desktop` | macOS/Windows/Linux ports | ports, headless, audio_cpal |
| `platform_ios` | iPadOS ports | ports, headless, audio_cpal |
| `platform_winit` | UI shell (winit + wgpu + libgui) | libgui |

## Decisions

### D1. Time is integer ticks, 1/254 016 000 000 s
Every common frame and sample rate (23.976 … 120 fps, 44.1 … 192 kHz)
divides that base exactly, so frame boundaries are integers and edits never
drift; `i64` spans ±1.1 years. Floats appear only at the UI edge.
*Rejected:* `f64` seconds (drift, non-associative sums); arbitrary rationals
(denominators grow under mixed-rate arithmetic and overflow).

### D2. The document is an immutable value
`Project` uses persistent collections (`imbl`) and `Arc`s; an edit produces a
new `Project` sharing everything it did not touch. The UI, playback and export
each hold their own snapshot without locks.

### D3. Every change is a serializable `Command` with an inverse
Undo stores inverses, not copies. The same commands serve the UI, scripts,
add-ons, autosave journals and (later) collaboration. A command applies fully
and leaves a valid project (`ve_model::validate`) or fails and changes nothing.

### D4. Stable ids, and media by reference, not path
Every object has a ULID. Media is a `MediaRef` interpreted only by the
`Storage` port: a path on desktop, a security-scoped bookmark on iPad.

### D5. Versioned project file with migrations
`{ "format": "ve-project", "schema_version": N, "project": … }`. Older files
migrate step by step; newer files are refused rather than half-read.

### D6. Rendering is a pure function of (snapshot, time, quality)
`evaluate()` decides what is visible; the compositor draws it. Playback,
scrubbing, thumbnails and export share it, so they cannot disagree.

### D7. Pixels: linear-light RGBA16F, OpenColorIO for colour
The working space is scene-linear (`SequenceFormat::working_space`, default
ACEScg). Decoders tag frames with their colour (`ColorTags`); the display
transform happens once, at the monitor. Retrofitting colour management later
is a rewrite, so it is decided now. (OCIO wiring: Phase C.)

### D8. wgpu is the GPU layer everywhere, not a port
Metal on Apple, Vulkan/D3D12 elsewhere. The UI, the compositor and decoder
imports share one device, so decoded frames reach the monitor without copies.
Only frame *import* is platform-specific (`TextureImporter`).

### D9. The audio device is the master clock
The playhead while playing is read from frames the device has played, so
picture follows sound. The monotonic clock is the fallback without audio.

### D10. Capabilities, not OS names
The engine asks `System::capabilities()` — can I start processes? load code
from files? JIT? how much memory? — and adapts. A new platform is a new
adapter, never an engine change.

### D11. Frontend and engine meet only at the engine API
Commands in; snapshots, frame plans and events out. Frontends never hold a
mutable project. Today the engine runs on the caller's thread; moving it to
its own thread (or behind a socket) is internal to `ve_engine`.

### D12. Plugins: several APIs, one registry

| API | Runs | Platforms | For |
|---|---|---|---|
| **`ve` native** (`ve_plugin.h`) | in process | all — linked in on iPadOS | effects, transitions, generators; later codecs, importers |
| **OpenFX 1.5** | in process | desktop | the existing video-effect ecosystem (Resolve, Nuke, Natron plugins) |
| **CLAP** | in process | desktop | audio effects |
| **WASM add-ons** | in process, sandboxed | all — interpreted on iPadOS | third-party tools, scripts, automation |
| **Process add-ons** | separate process | where `processes` | AI models, heavy tools, other languages |

Why both `ve` and OpenFX: OpenFX brings hundreds of existing plugins but is
large, CPU-centric in practice, and cannot load on iPad. The `ve` ABI is small,
GPU-first (an effect can be **just a WGSL shader** — no native code, so it runs
on iPad), and also covers what OpenFX does not (codecs, importers).

Why WASM *and* processes: iPadOS forbids child processes, so the
crash-isolated tier must also exist in-process. Both speak the same add-on
protocol, so an add-on author targets one API.

ABI rules (`ve_plugin.h`): C only; `struct_size` first in every struct and
append-only growth; extensions by name; the host owns parameters, keyframes,
undo and UI; statically linked plugins export `ve_plugin_entry_<name>`.
Built-ins use the same ABI (`ve_builtins`), so we are its first user.

### D13. iPadOS is a first-class target from day one
Every phase is done only when it works on iPad too. What iPadOS forbids and
the engine's answer:

| Forbidden | Answer |
|---|---|
| child processes | `Platform::processes = None`; WASM add-ons instead |
| loading code from files | plugins linked in; `NativeLibraries::linked()` |
| JIT | WASM interpreted |
| arbitrary paths | `MediaRef` + security-scoped bookmarks |
| unbounded memory | budgets from `Capabilities`, eviction on memory warnings |

### D14. Third-party code is pinned, vendored as source, built by script
See [third_party/README.md](../third_party/README.md). FFmpeg is built **LGPL**
(no GPL/nonfree parts; encoding through VideoToolbox). C++ libraries (OCIO,
OTIO) will sit behind a thin C shim and a `-sys` crate, then a safe wrapper; no
C++ type crosses a crate boundary.

## Threads (target design)

| Thread | Does | Rule |
|---|---|---|
| UI | input, layout, drawing | never blocks on I/O or decode |
| Engine | applies commands, publishes snapshots | owns the history |
| Decode pool | demux + decode ahead of the playhead | budgeted by `Capabilities` |
| GPU submit | compositor, uploads | one queue shared with the UI |
| Audio (real-time) | mixes into the device buffer | no allocation, locks or waits |

Since Phase B the engine runs on its own thread (`Engine::spawn` →
`EngineClient`). The UI reads the latest `Published` state under a short lock
and computes the playhead itself from the transport and the audio device's
shared frame counter; commands, imports and saves are sent and never waited on.
The engine thread wakes the UI through the shell's `Waker`.

### D15. Edit tools compile to primitive commands
Razor, ripple delete/trim, roll, slip, insert, overwrite and linked moves are
functions in `ve_command::edit` that build a `Batch` of primitives, applying
each step to a scratch project as they go (`Builder`). The timeline previews a
drag by applying the very command it will send, so preview and result cannot
differ. Every tool is tested to undo exactly.

### D17. Pictures flow as tagged frames; colour is decided in the compositor
Decoders hand out NV12 or P010 with their colour tags; the compositor's
decode pass is the only place that interprets them. Every source is
converted to the sequence's working space (scene-linear, ACEScg by default)
and only the display pass makes it a monitor signal — so mixed sources
(709, 2020/PQ, sRGB stills) composite correctly, and export encodes exactly
what the monitor shows.

### D18. Picture and sound are slaved to one clock
The audio device's played-frame counter drives the transport; video shows the
frame at that time from the cache, never blocking. Export uses the same
`evaluate → resolve → composite` path but waits for exact frames, and mixes
audio per video frame in whole samples, so the two cannot drift.

### D16. Intrinsic effects are described like plugins
Motion, Opacity, Volume and Panner are `EffectInfo`s with
`Implementation::Intrinsic`, attached to new clips by `make_clip`. Effect
Controls is generated from `ParamInfo`, so intrinsic and plugin effects get
the same stopwatch, keyframes, reset and undo.

## Phases

| Phase | Done when (on desktop **and** iPad) |
|---|---|
| **A — skeleton** ✅ | Every crate exists; ports defined; headless tests; FFmpeg probe; a C plugin loads statically; the UI shows a real project; iPad simulator runs |
| **B — the spine** ✅ | Engine on its own thread; edit tools (razor, ripple, roll, slip, insert, overwrite, linked moves, snapping) as commands; `libgui_cut` look ported with generated Effect Controls; multi-window docking on desktop; native file dialogs; iPad document picker (import-copy) and memory warnings. *Carried to C:* opening media in place on iPad via security-scoped bookmarks |
| **C — the media path** ✅ | FFmpeg decode (VideoToolbox) → budgeted decode-ahead frame cache → wgpu compositor (YUV → scene-linear ACEScg, Motion, Opacity, blend modes, plugin WGSL effects, Rec.709 display) → monitor; audio mixer (volume, pan, mute, solo) on a lock-free ring at the device's own rate, with real meters; export to H.264 / HEVC (VideoToolbox) or ProRes 422 HQ + AAC; real thumbnails and waveforms. *Carried forward:* VideoToolbox → Metal zero-copy (frames are copied to memory today); OpenColorIO (built-in 709/2020/sRGB/PQ/HLG maths today); Overlay blend; audio during reverse/shuttle; output-latency compensation; title/generator rendering; CPU-only native effects; iPad security-scoped bookmarks |
| **D — plugin APIs v1** | `ve` ABI frozen with GPU path; OpenFX host; CLAP host; WASM add-on runtime + protocol; process add-ons on desktop |
| **then** | features, on top |
