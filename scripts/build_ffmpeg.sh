#!/usr/bin/env bash
# Build the pinned FFmpeg as an LGPL static library for one target.
#   scripts/build_ffmpeg.sh <target>
#     macos-arm64 | macos-x86_64 | ios-arm64 | ios-sim-arm64     (on a Mac)
#     linux-x86_64 | linux-arm64                                 (on Linux, natively)
#     windows-x86_64 | windows-arm64                             (MSYS2 shell + MSVC, natively)
# Output: third_party/_build/ffmpeg/<target>/{include,lib}
#
# LGPL only: no --enable-gpl, no --enable-nonfree. Video is decoded and
# encoded on the GPU's media engines where the machine has them, through
# APIs that need no GPL or non-free code:
#   Apple    VideoToolbox (decode + encode)
#   Windows  D3D12VA / D3D11VA / DXVA2 decode (any GPU), NVDEC + NVENC
#            (NVIDIA), Media Foundation encode (any GPU's encoder: AMD, Intel,
#            NVIDIA)
#   Linux    VA-API decode + encode (Intel, AMD), NVDEC + NVENC (NVIDIA),
#            Vulkan Video decode + encode
# NVIDIA support loads the driver at run time from the MIT-licensed
# nv-codec-headers (third_party/nv-codec-headers); nothing NVIDIA is linked.
# Before shipping on iOS, switch to shared frameworks so the LGPL relinking
# requirement is met.
#
# Prerequisites off Apple:
#   Linux    a C toolchain, nasm (x86_64), pkg-config, make; libva-dev
#            (VA-API), zlib1g-dev. Vulkan headers come pinned from
#            third_party/vulkan-headers (distributions' are often older than
#            FFmpeg needs).
#   Windows  MSYS2 with make, diffutils, nasm (x64) and pkgconf, started with
#            MSVC's environment (a "Native Tools" prompt for the target
#            architecture) and inheriting its PATH, so cl.exe and MSVC's
#            link.exe are found — rename MSYS2's /usr/bin/link.exe first, it
#            shadows MSVC's. ARM64 builds without assembly (MSVC cannot
#            assemble FFmpeg's AArch64 sources); decoding is on the GPU there.
set -euo pipefail
target="${1:?target}"
root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/third_party/ffmpeg"
out="$root/third_party/_build/ffmpeg/$target"
work="$root/third_party/_build/ffmpeg-work/$target"
jobs="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 4)"

common=(--enable-static --disable-shared --enable-pic
        --disable-programs --disable-doc --disable-debug --disable-avdevice
        --disable-autodetect)
extra=()

# NVIDIA's MIT headers, installed into the work dir for pkg-config.
nvidia() {
  local hdr="$root/third_party/nv-codec-headers"
  [[ -f "$hdr/Makefile" ]] || { echo "missing $hdr: run scripts/fetch_third_party.sh" >&2; exit 1; }
  make -C "$hdr" install PREFIX="$work/ffnvcodec" >/dev/null
  export PKG_CONFIG_PATH="$work/ffnvcodec/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
  extra+=(--enable-ffnvcodec --enable-nvdec --enable-nvenc)
}

# x86 assembly needs nasm; without it FFmpeg builds but decodes in software
# much more slowly, so say so.
x86asm() {
  if ! command -v nasm >/dev/null; then
    echo "warning: nasm not found; building without x86 assembly (slow software decode)" >&2
    extra+=(--disable-x86asm)
  fi
}

case "$target" in
  macos-arm64|macos-x86_64|ios-arm64|ios-sim-arm64)
    case "$target" in
      macos-arm64)   sdk=macosx;          arch=arm64;  min="-mmacosx-version-min=12.0" ;;
      macos-x86_64)  sdk=macosx;          arch=x86_64; min="-mmacosx-version-min=12.0"; x86asm ;;
      ios-arm64)     sdk=iphoneos;        arch=arm64;  min="-miphoneos-version-min=17.0" ;;
      ios-sim-arm64) sdk=iphonesimulator; arch=arm64;  min="-mios-simulator-version-min=17.0" ;;
    esac
    # Cross-compiling whenever the target is not this Mac itself.
    if [[ "$sdk" != macosx || "$arch" != "$(uname -m)" ]]; then
      extra+=(--enable-cross-compile --target-os=darwin)
    fi
    sysroot="$(xcrun --sdk "$sdk" --show-sdk-path)"
    # Host tools (built and run during the build) always target this Mac.
    host_sysroot="$(xcrun --sdk macosx --show-sdk-path)"
    extra+=(--arch="$arch" --cc="$(xcrun --sdk "$sdk" -f clang)" --sysroot="$sysroot"
            --host-cflags="-isysroot $host_sysroot" --host-ldflags="-isysroot $host_sysroot"
            --extra-cflags="-arch $arch $min" --extra-ldflags="-arch $arch $min"
            --enable-zlib --enable-videotoolbox --enable-audiotoolbox)
    ;;
  linux-x86_64|linux-arm64)
    want="${target#linux-}"; want="${want/arm64/aarch64}"
    [[ "$(uname -s)" == Linux && "$(uname -m)" == "$want" ]] || { echo "$target builds natively on a $want Linux machine" >&2; exit 1; }
    [[ "$want" == x86_64 ]] && x86asm
    nvidia
    vk="$root/third_party/vulkan-headers/include"
    [[ -f "$vk/vulkan/vulkan.h" ]] || { echo "missing $vk: run scripts/fetch_third_party.sh" >&2; exit 1; }
    extra+=(--enable-zlib --enable-vaapi --enable-vulkan --extra-cflags="-I$vk")
    ;;
  windows-x86_64|windows-arm64)
    command -v cl >/dev/null || { echo "$target builds in MSYS2 with MSVC on PATH (Native Tools prompt)" >&2; exit 1; }
    if [[ "$target" == windows-x86_64 ]]; then
      x86asm
      nvidia # no NVIDIA GPUs on Windows on ARM
      extra+=(--arch=x86_64)
    else
      extra+=(--arch=aarch64 --disable-asm)
    fi
    extra+=(--toolchain=msvc --target-os=win64
            --enable-d3d11va --enable-d3d12va --enable-dxva2 --enable-mediafoundation)
    ;;
  *) echo "unknown target $target" >&2; exit 1 ;;
esac

mkdir -p "$work" && cd "$work"
"$src/configure" --prefix="$out" "${common[@]}" "${extra[@]}" \
  >configure.out || { tail -5 configure.out; tail -20 ffbuild/config.log; exit 1; }
make -j"$jobs" >/dev/null
make install >/dev/null
echo "ffmpeg $target -> $out"
