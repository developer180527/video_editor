#!/usr/bin/env bash
# Run the desktop app in one state and measure it the way the battery sees it.
#   scripts/profile_app.sh <label> [media|project] [play] [template]
# Settles 8 s, then reads the kernel's accounting over 10 s (scripts/rusage.py:
# CPU, wakeups/s, instructions, energy, footprint). With a template ("Time Profiler",
# "System Trace", "Allocations", "Power Profiler"…) it then records 10 s with
# Instruments into target/traces/<label>.trace.
# Uses target/prof (release + line tables), building it if missing.
set -euo pipefail
cd "$(dirname "$0")/.."
label=$1; what=${2:-}; play=${3:-}; template=${4:-}
app=target/prof/release/video-editor
[[ -x $app ]] || CARGO_PROFILE_RELEASE_DEBUG=line-tables-only CARGO_TARGET_DIR=target/prof \
  cargo build --release -p editor_desktop >/dev/null
args=(); envs=()
case $what in
  "") ;;
  *.veproj) args=("$what") ;;
  *) envs+=("VE_PROFILE_MEDIA=$(cd "$(dirname "$what")" && pwd)/$(basename "$what")") ;;
esac
[[ $play == play ]] && envs+=(VE_PROFILE_PLAY=1)
env ${envs[@]+"${envs[@]}"} "$app" ${args[@]+"${args[@]}"} >/dev/null 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null || true' EXIT
sleep 8
printf "%-14s " "$label"; python3 scripts/rusage.py "$pid" 10
if [[ -n $template ]]; then
  mkdir -p target/traces
  out="target/traces/$label.trace"
  rm -rf "$out"
  xcrun xctrace record --quiet --template "$template" --attach "$pid" --time-limit 10s --output "$out" >/dev/null 2>&1
  echo "trace: $out"
fi
