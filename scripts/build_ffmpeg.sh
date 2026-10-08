#!/usr/bin/env bash
# Build the pinned FFmpeg as an LGPL static library for one target.
#   scripts/build_ffmpeg.sh macos-arm64 | ios-arm64 | ios-sim-arm64
# Output: third_party/_build/ffmpeg/<target>/{include,lib}
#
# LGPL only: no --enable-gpl, no --enable-nonfree. Encoding goes through
# VideoToolbox. Before shipping on iOS, switch to shared frameworks so the
# LGPL relinking requirement is met.
set -euo pipefail
target="${1:?target}"
root="$(cd "$(dirname "$0")/.." && pwd)"
src="$root/third_party/ffmpeg"
out="$root/third_party/_build/ffmpeg/$target"
work="$root/third_party/_build/ffmpeg-work/$target"

case "$target" in
  macos-arm64)   sdk=macosx;          min="-mmacosx-version-min=12.0"; cross=() ;;
  ios-arm64)     sdk=iphoneos;        min="-miphoneos-version-min=17.0"
                 cross=(--enable-cross-compile --target-os=darwin) ;;
  ios-sim-arm64) sdk=iphonesimulator; min="-mios-simulator-version-min=17.0"
                 cross=(--enable-cross-compile --target-os=darwin) ;;
  *) echo "unknown target $target" >&2; exit 1 ;;
esac
sysroot="$(xcrun --sdk "$sdk" --show-sdk-path)"
cc="$(xcrun --sdk "$sdk" -f clang)"
# Host tools (built and run during the build) always target this Mac.
host_sysroot="$(xcrun --sdk macosx --show-sdk-path)"

mkdir -p "$work" && cd "$work"
"$src/configure" \
  --prefix="$out" ${cross[@]+"${cross[@]}"} \
  --arch=arm64 --cc="$cc" --sysroot="$sysroot" \
  --host-cflags="-isysroot $host_sysroot" --host-ldflags="-isysroot $host_sysroot" \
  --extra-cflags="-arch arm64 $min" --extra-ldflags="-arch arm64 $min" \
  --enable-static --disable-shared --enable-pic \
  --disable-programs --disable-doc --disable-debug \
  --disable-avdevice \
  --disable-autodetect --enable-zlib --enable-videotoolbox --enable-audiotoolbox \
  >configure.out || { tail -5 configure.out; tail -20 ffbuild/config.log; exit 1; }
make -j"$(sysctl -n hw.ncpu)" >/dev/null
make install >/dev/null
echo "ffmpeg $target -> $out"
