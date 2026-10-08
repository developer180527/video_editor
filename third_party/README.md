# third_party

Sources are **not** committed. `scripts/fetch_third_party.sh` clones each one
at the ref pinned in [`PINS`](PINS); bump a pin, rerun, rebuild. Build output
goes to `_build/` (also not committed).

| Name | Pinned | Licence | Used for | Status |
|---|---|---|---|---|
| libgui | `da04dc8` | MIT / Apache-2.0 | the UI | in use (path deps) |
| ffmpeg | `n9.0.2` | **LGPL-2.1+** (built without GPL/nonfree) | demux, decode, encode | in use: `scripts/build_ffmpeg.sh` → `_build/ffmpeg/<target>` |
| opencolorio | `v2.6.0` | BSD-3 | colour management | Phase C |
| opentimelineio | `v0.18.1` | Apache-2.0 | interchange (FCP XML, EDL, AAF via adapters) | Phase B/C |
| openfx | `OFX_Release_1.5.1` | BSD-3 | OpenFX host headers | Phase D |
| clap | `1.2.10` | MIT | CLAP host headers | Phase D |

## Building FFmpeg

```bash
scripts/build_ffmpeg.sh macos-arm64      # desktop dev
scripts/build_ffmpeg.sh ios-sim-arm64    # iPad simulator
scripts/build_ffmpeg.sh ios-arm64        # iPad device
```

Static, LGPL, VideoToolbox/AudioToolbox on, zlib on, nothing else
autodetected. `crates/media_ffmpeg/build.rs` links whichever matches the
target (`FFMPEG_DIR` overrides).

**Before shipping on iOS:** LGPL requires that users can relink against a
modified FFmpeg. Ship FFmpeg as dynamic frameworks inside the app bundle
(switch `--enable-static` to `--enable-shared` and embed), or get legal
sign-off on the static build.

## How native libraries enter Rust

```
C++ library (OCIO, OTIO)       C library (FFmpeg)
        │ thin extern "C" shim         │
        ▼                              ▼
   -sys crate: bindgen over the C headers, build.rs links the _build output
        ▼
   safe wrapper crate (Rust types, Drop, Result) — the only thing others use
        ▼
   implements a port trait (e.g. MediaBackend) so it can be swapped
```

No C++ type crosses a crate boundary. Each library is built per target by a
script here (CMake for the C++ ones), so iOS and desktop builds come from the
same pinned source.
