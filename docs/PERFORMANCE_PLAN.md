# Energy and performance — measurements and work orders

**Status: WO-1 to WO-8 done** (2026-10-11); results below. WO-9 needs a PC.

## Results

| | Before | After |
|---|---|---|
| Mac, playing UHD HEVC: UI frames/s | 121 | **24** (the sequence's rate) |
| Mac, playing UHD HEVC: CPU | 33–42 % | **18–20 %** |
| Mac, playing 1080p: CPU | 42 % | **15 %** |
| Mac, idle · paused (settled) | 0.4 % · 0 % | 0.3 % · 0.4 % |
| Mac, playing UHD HEVC + blur, grade, glow, title, dissolve | can't hold 60 fps; GPU 20 ms/frame | **every frame**; GPU 6.6 ms (1/2), 9.3 ms (full); ~31 % CPU |
| First paused frame with effects | 295 ms | 35 ms; shaders compile ahead in the background |
| Export ProRes (bench) | 1 core, 0.7–0.8× real time | **~10 cores, 5.3–6.0× real time** (4.0× with effects) |
| **iPad Pro M5, playing UHD HEVC** | **121 frames/s, 62–75 % CPU** | **24–25 frames/s, 13–16 % CPU**, peak 735 MB |
| Icon path hashing | ≈25 % of all CPU | gone from the profile |

What was done:

- **WO-1** — the UI is rebuilt when something visible changes: while
  playing, at each frame boundary of the picture (faster when shuttling;
  the audio clock's steps are allowed for), otherwise at 30 Hz only while
  meters fall, a frame decodes, thumbnails arrive or work runs. The
  monitors and scopes redraw only when their picture changes (document,
  time, size, decoded pictures). The decode pool wakes the UI only when a
  viewer was shown a stand-in. The shell builds when a requested wake is
  due (two clocks disagreeing had made it spin).
- **WO-2** — libgui paths fold each segment into a hash as they are built;
  the per-frame key only mixes in size, rule and view box.
- **WO-3** — thumbnail and waveform work runs as a background thread
  (Apple: utility QoS, the efficiency cores; Linux: nice 10; Windows:
  background mode), through a `ve_ports` hook.
- **WO-4** — a layer with effects shown much smaller than stored is
  filtered down once (a short mip chain and a trilinear pass) and its
  effects run at the size seen. `params.scale` says so; the contract's
  wording was updated.
- **WO-5** — mip chains are built only before shaders that can sample
  below level 0, found with naga when an effect loads
  (`wgsl::reads_lower_levels`). The built-in dissolve, dip and grade read
  level 0 explicitly.
- **WO-6** — one `EffectPipelines` cache per device, shared by both
  monitors, warmed on a background thread whenever the document changes.
- **WO-7** — encoders and the RGB → YUV conversion use every core.
- **WO-8** — iOS memory warnings purge the decode pool and thumbnails
  (`ve_ports::on_memory_warning`); peak memory is reported by
  `VE_PROFILE_FPS`.

Measuring note: the first few launches of a freshly built binary cost a
core or more for many seconds on macOS (caches rebuilding) — discard them.

**Measured 2026-10-11** on a MacBook Pro, Apple M4 Pro (12 cores, 24 GB),
built-in ProMotion display (120 Hz), and an iPad Pro M5 (see the end).

## How to measure (repeatable)

| Tool | What it gives |
|---|---|
| `ve bench [--only NAME] [--secs N]` | The engine and GPU compositor as the Program monitor drives them (60 Hz ticks, real audio clock): CPU cores used, frames drawn, late frames, frame and GPU time p50/p95; scrubbing; ProRes export speed; peak memory. Makes its own media and projects (`target/bench/*.veproj`). |
| `scripts/measure_app.sh <label> [project] [play]` | The real app: mean CPU % and macOS energy impact over 12 s, after 6 s settling. |
| `VE_PROFILE_FPS=1` | UI frames built and presented per second, per window, and the process's CPU % — works on the iPad too. |
| `VE_PROFILE_PLAY=1`, `VE_PROFILE_MEDIA=<file>` | Start playing (looping) / put a file on the timeline at launch, so Instruments can record untouched — on the Mac or the iPad (`devicectl … --environment-variables`). |
| Instruments: Time Profiler, Power Profiler, Metal System Trace | Where the time goes; `xctrace record --attach <pid>`. Power Profiler records on iOS/iPadOS only. |
| `scripts/profile_app.sh <label> [media\|project] [play] [template]` | The app in one state: the kernel's own accounting over 10 s after 8 s settling (`scripts/rusage.py`: CPU, idle and interrupt wakeups/s, instructions, energy in mW, footprint), then optionally 10 s of an Instruments template into `target/traces/<label>.trace`. Builds `target/prof` (release + line tables). `top`'s IDLEW column reads 0 on macOS 27 (even for a 1 kHz sleeper); `rusage.py` does not. |
| `scripts/tprof.py <trace> [N] [--thread S] [--focus S]` | A Time Profiler trace summarised in the terminal: threads, top self and inclusive functions. |
| `scripts/gpu_summary.py <trace>` | A Metal System Trace: GPU time by pass, GPU busy %, command buffers per second. |
| `VE_PROFILE_FPS=1` (also prints) / `VE_DAMAGE_DEBUG=1` | How much of the window each present repainted / the damage rectangles themselves. |

## Baseline

`ve bench --secs 8` (sequence 1920×1080 at 24 fps; clips 30 fps):

| Case | State | Cores | Drawn/s | GPU p50 / p95 |
|---|---|---|---|---|
| 1080p H.264 | idle / paused | 0.00 / 0.02 | – | – |
| | playing 1/2 · full | 0.19 · 0.17 | 60 · 60 | 1.0 / 1.2 ms |
| | scrubbing | 0.22 | 30 (all late) | 1.0 ms |
| UHD H.264 | playing | 0.24 | 60 | 2.0 / 2.6 ms |
| UHD HEVC 10-bit | playing | 0.33 | 60 | 2.3 / 3.1 ms |
| UHD HEVC + blur, grade, glow, title, dissolve | playing | 0.25 | **45 (can't keep 60)** | **19.9 / 20.5 ms** |
| | first paused frame | – | – | **295 ms** total |
| any | export ProRes | **1.0** | **0.7–0.8× real time** | – |

Peak memory in the bench: **2.2 GB**.

The app (`measure_app.sh`, Time Profiler, `VE_PROFILE_FPS`):

| State | CPU | Notes |
|---|---|---|
| idle, settled | 0.4 % | good |
| project open, paused, settled | 0 % after ~4 s | first open of a clip spends a few seconds of a core on thumbnails (cached after) |
| playing UHD HEVC | **33–42 %** | **121 UI frames built and presented per second** for 24 fps video |

Where playback's CPU goes (Time Profiler, 10 s, UHD HEVC):

- Main thread 60 %: UI build 39 % — of which **libgui `StableHasher` +
  `Path::key` ≈ 25 % of all CPU** (the vector icons' cache key re-hashes
  every point of every icon on every frame); wgpu command recording ≈ 22 %;
  compositor ≈ 9 %.
- A Metal driver thread 36 %: `IOGPUCommandQueueSubmitCommandBuffers` — it
  scales with frames submitted per second.
- Decoding < 2 % (hardware), mixer 2 %.

## What the numbers say

1. **The picture is redrawn far more often than it changes.** The UI runs at
   the display's 120 Hz while playing, rebuilding everything and
   recompositing the monitor each time, for a 24 fps picture: ~5× the work.
2. **Icons cost a quarter of the CPU** through re-hashing their paths.
3. **Effects run at the source's resolution.** A 4K clip in an HD sequence
   is blurred and glowed at 3840×2160 even in a half-size preview (16× the
   pixels shown).
4. **Every effect pass builds a mip chain** whether its shader reads one or not.
5. **The first use of an effect compiles its shader on the spot** (a 295 ms hitch).
6. **Export is serial and single-threaded**: one core, slower than real time.

## Work orders

Each ends with the same measurement re-run; numbers go in this file.

### Batch A — stop doing work nobody sees (energy)

**WO-1 Draw at the picture's rate.** While playing, the UI is rebuilt when
something visible changes, not every display refresh: the monitor when the
sequence frame index changes (or a late frame arrives), the playhead and
timecode at the sequence's frame rate (at least 30 Hz, so motion stays
smooth), meters at 30 Hz. Recompositing keys on (sequence frame, quality,
document revision), not on the playhead's exact time. ProMotion displays
drop their refresh rate by themselves when we present less.
*Target:* 121 → ~24–30 frames/s while playing; app CPU 36 % → ~10 %;
fewer Metal submits.

**WO-2 Icons hash once.** libgui's `Path` keeps its content hash, computed
when the path is built; `key(w, h)` combines it with the size (a libgui
change). The icon stroker drops discs at collinear joints and uses fewer
segments per disc.
*Target:* `StableHasher` and `Path::key` gone from the profile (−25 % CPU
whenever the UI draws).

**WO-3 Background work yields.** Thumbnail and waveform generation run at
utility / background QoS (efficiency cores on Apple silicon), throttled
while playing.

### Batch B — less GPU work per frame (performance and energy)

**WO-4 Effects at the size they are seen.** Before a layer's effects, its
picture is reduced to the size it is shown at (its motion scale × the
output's scale), never larger than the source; `params.scale` already tells
shaders the texels per picture pixel, so effects look the same.
*Target:* the effects case from 20 ms to ≲ 5 ms per frame at full
quality; 60 fps held.

**WO-5 Mip chains only when read.** An effect's shader is checked when it
loads for reading lower levels (`textureSampleLevel` with a level other
than 0, or `textureSample`); only those get a chain built before them.

**WO-6 No hitch on first use.** Effect pipelines compile in the background
when a project opens or an effect is added, not on the first frame that
shows them.
*Target:* first paused frame with effects ≲ 50 ms.

### Batch C — throughput

**WO-7 Export in parallel.** Decode, render, read back and encode overlap
(a pipeline of a few frames); encoders that can thread (ProRes, software
codecs) get slice threads.
*Target:* ProRes export ≥ 2.5× real time on this machine.

**WO-8 Memory.** Measure on the iPad; size the frame cache to the device
(the bench peaks at 2.2 GB); trim caches on iOS memory warnings (nothing
reacts to them today, so iOS would kill the app instead).

### Later (needs other machines)

**WO-9 Zero-copy on Windows and Linux** (D3D11/12, VA-API/Vulkan frames
into the compositor without the CPU copy Apple already avoids). Needs a PC
to measure.

## iPad Pro M5 (iPadOS 27.2, ProMotion)

Instruments cannot attach to iPadOS 27.2 yet ("waiting for device to
boot"), so the app measures itself: `VE_PROFILE_FPS=1` prints frames and
the process's CPU (getrusage) each second; launched over the cable with
`devicectl … --environment-variables` and the test clip copied into the
app's Documents.

| State | Frames/s | CPU |
|---|---|---|
| UHD HEVC 10-bit, paused (after loading) | 0 (nothing drawn) | – |
| UHD HEVC 10-bit, playing | **121** | **62–75 %** (about twice the Mac's 36 %) |

The same story as the Mac, at twice the cost: WO-1 and WO-2 matter most here.

## Second round (2026-10-13): the newer features, and real files

Measured on the same MacBook Pro (M4 Pro): the app (`top`: CPU % of one
core and macOS energy impact, mean over 12 s after settling) and
`ve bench --secs 8`.

| State | Before | After |
|---|---|---|
| Idle · project open, paused | 0.4 % · 0.4 % | — |
| Playing 1080p · UHD H.264 · UHD HEVC 10-bit | 14.3 · 17.4 · 17.5 % | — |
| UHD HEVC + blur, grade, glow, title, dissolve | 29.4 % | — |
| 1080p + built-in audio chain (HPF, EQ, comp, limiter, reverb) | 14.5 % (+0.01 cores in the bench) | — |
| 1080p + two CLAP effects | 16.9 % (+0.04 cores) | — |
| 2 h H.264 movie playing | 14.7 % | — |
| **iPhone HDR clip (VFR, with an APAC track), paused** | **33.9 %, forever** | **0.4 %** |
| **iPhone HDR clip playing (60 fps)** | **44.5 %** | **15.0 %** |
| **Opening the 2 h movie again (first 15 s)** | **44 % (peaks 105 %)** | **13.8 %** |
| Startup with CLAP plugins installed | 0.06 s (0.05 without) | — |

What was found and fixed:

- **A waveform that cannot be made was asked for on every frame.** iPhones
  record a second audio track in Apple's APAC, which no decoder here reads;
  its waveform failed, the failure was not remembered, the UI was woken,
  asked again… 121 UI rebuilds a second while paused. Failures are now
  remembered, and audio tracks that cannot be decoded get no clips
  (`AudioStreamInfo::decodable`).
- **Integrated loudness rescanned the whole playback on every mixer block**
  (allocating), so the meter cost grew with time: 21 % of the mixer after
  10 s, linear after that. Now a histogram of block loudness (0.1 LU bins):
  constant cost.
- **Waveforms were decoded again on every launch** (a feature's soundtrack:
  a core for ~15 s). They are cached in Caches/VideoEditor/Waveforms, keyed
  by file, size and modification time.

Where 1080p playback's ~17 % of a core goes (Time Profiler, 10 s): main
thread 37 % (compositor encoding 17 %, the UI 20 %), Metal submission on
system threads ~25 %, the mixer 10 % (resampling 23 % of it, the loudness
meter 21 % before the fix), decoding 5 %, thumbnails 3 %.

Left as is: scrubbing that jumps 15 times a second across a file (the
bench's synthetic case) shows stand-ins 100+ frames away — real drags (the
"dragging" rows) stay within 0–2 frames forwards, 11 backwards at p95.

## Third round (2026-11-09): Instruments, energy first

Measured on the MacBook Pro (M4 Pro) with `scripts/profile_app.sh` (kernel
accounting: `proc_pid_rusage`), Metal System Trace and Time Profiler.
Playback output: Bluetooth headphones (44.1 kHz, buffers up to 1024 frames).

| State | Before | After |
|---|---|---|
| Idle · paused (1080p, UHD HEVC) | 0 % CPU, 0.4 wakeups/s | same (no work at all: nothing presented while paused) |
| **Paused UHD HEVC: memory** | **980 MB** (peak 1.17 GB) | **450 MB** |
| **Playing UHD HEVC: memory** | **1.18 GB** | **545 MB** |
| **Playing UHD HEVC: GPU busy** | **12.2 %** (122 ms/s) | **6.3 %** (63 ms/s) |
| Playing: UI fragment work | 94 ms/s (the whole window, 3.8× its area) | 29 ms/s (23 % of the window: the monitor, playhead, timecode, meters) |
| Playing: command buffers | 194/s (8 a frame) | 146/s (6 a frame) |
| **Playing: wakeups** | **97/s** | **47/s** |
| Playing UHD HEVC: CPU · energy (process) | 19–20 % · 31–34 mW | 17.3 % · 29 mW |
| Playing 1080p: CPU · energy | 15–17 % · 22–26 mW | 15.5 % · 24 mW |
| Export (`ve bench --jobs`) | — | unchanged (H.264 7.5×, HEVC 7.5×, ProRes 5.6× on UHD H.264) |

What was found and fixed:

- **Hardware decoders ran 13 frame threads each.** `thread_count = 0` gave
  every VideoToolbox decoder a frame-thread context per core, each with its
  own copy of the HEVC tables (~10 MB a thread at UHD): 550 MB of a paused
  UHD project, 27 extra threads, and no speed (FFmpeg serialises the
  hardware). Hardware decoders now use one thread; a stream the hardware
  refuses after all moves to the threaded software decoder. What is left
  is FFmpeg's per-picture HEVC side data (12 × 10.5 MB at UHD) — FFmpeg's.
- **The whole window was shaded again for every picture.** The shell now
  diffs each frame's draw list against the last one drawn
  (`platform_winit::damage`): instances in one frame and not the other, and
  images whose texture got a new picture (`ShellApp::textures_changed`),
  damage where they cover; only that is repainted (scissored, with only
  the instances that reach each rectangle) into a kept canvas, which is
  copied to the swapchain image (0.35 ms a frame). A frame built with
  nothing different presents nothing. `editor_app/tests/damage.rs` renders
  real editor frames before and after changes (playhead, hover, selection,
  scrolling, a dialog) on the CPU and checks that every changed pixel is
  inside the damage.
- **The UHD picture was decoded whole into a float texture, then reduced.**
  A decoded picture without effects shown below half size is decoded
  straight to the mip level it is read at — each pixel the linear-light
  mean of the texels under it, which is what the chain made: no 66 MB
  intermediate, no chain passes (5 → 3 passes a frame, −90 MB). The GPU
  time is unchanged (the transfer function's `pow` dominates either way);
  the saving is memory and DRAM traffic. `small_pictures_decoded_small_match_the_whole_decode`
  holds it to ±1 code value against the old path.
- **Per-frame GPU plumbing.** Uniforms are written only when they change
  (each `write_buffer` is a staging buffer and a copy); a target's clear
  rides in the first pass that draws over it; the monitors' commands go in
  the frame's one submission (`ShellApp::commands`).
- **Audio woke the CPU ~4 times per device callback.** The mixer slept
  "until the ring drains to the target" as if the device drained smoothly,
  but it takes its buffer in bursts. It now fills to 250 ms ahead, sleeps
  until 100 ms has played and mixes that in one go (~10 wakes a second;
  an edit is still heard within 250 ms). The output asks for 2048-frame
  buffers where the device allows (built-in output; Bluetooth stops at
  1024). The clock now stamps each count with when it is *heard*
  (`RenderCallback` gets the output latency; cpal reports it), so the
  picture follows the sound at any buffer size — before, it ran ahead by a
  buffer plus the latency (most on Bluetooth).

Measured and left alone:

- **Allocation churn**: 6.7 % of playback CPU is the allocator and
  zeroing, nearly all inside Metal, FFmpeg's HEVC parser and XPC; our own
  code is under 0.5 %. Arenas would not pay.
- **File reads**: 0.4 % of a core, from the page cache; the kernel reads
  ahead for sequential files. A larger AVIO buffer would not be measurable.
- **Importing the decoded IOSurface planes** (≈5 % of process CPU): a
  texture per frame. Caching per IOSurface would keep VideoToolbox's pool
  buffers pinned; not done.
- **wgpu on Metal makes a command buffer per pass**; the per-pass driver
  cost (≈10 % of the main thread) is what fewer passes buy back.
