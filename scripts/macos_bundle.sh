#!/usr/bin/env bash
# Build the desktop editor and assemble target/macos/Video Editor.app.
#   open "target/macos/Video Editor.app" --args project.veproj
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release -p editor_desktop
app="target/macos/Video Editor.app"
rm -rf "$app" && mkdir -p "$app/Contents/MacOS"
cp target/release/video-editor "$app/Contents/MacOS/"
cp apps/editor_desktop/macos/Info.plist "$app/Contents/"
codesign --force --sign - "$app" >/dev/null 2>&1
echo "$app"
