#!/usr/bin/env bash
# Build the iPad app and assemble target/ios/<target>/VideoEditor.app.
#   scripts/ios_bundle.sh sim      → Simulator (aarch64-apple-ios-sim)
#   scripts/ios_bundle.sh device   → device (aarch64-apple-ios); sign it yourself
# Simulator: xcrun simctl install booted target/ios/sim/VideoEditor.app
#            xcrun simctl launch booted org.ve.videoeditor
set -euo pipefail
cd "$(dirname "$0")/.."
case "${1:-sim}" in
  sim)    triple=aarch64-apple-ios-sim; ff=ios-sim-arm64 ;;
  device) triple=aarch64-apple-ios;     ff=ios-arm64 ;;
  *) echo "usage: $0 sim|device" >&2; exit 2 ;;
esac
[[ -f third_party/_build/ffmpeg/$ff/lib/libavcodec.a ]] || scripts/build_ffmpeg.sh "$ff"
cargo build --release -p editor_ios --target "$triple"
app="target/ios/${1:-sim}/VideoEditor.app"
rm -rf "$app" && mkdir -p "$app"
cp "target/$triple/release/VideoEditor" "$app/"
cp apps/editor_ios/ios/Info.plist "$app/"
codesign --force --sign - "$app" >/dev/null 2>&1
echo "$app"
