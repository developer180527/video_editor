#!/usr/bin/env bash
# Run the desktop app in one state and sample its CPU and energy impact (as
# Activity Monitor reports it) once a second.
#   scripts/measure_app.sh <label> [project.veproj] [play]
# Settles for 6 s, then samples 12 s. "play" starts looping playback
# (VE_PROFILE_PLAY). Prints: label, mean CPU %, mean energy impact.
set -euo pipefail
cd "$(dirname "$0")/.."
label=$1; project=${2:-}; play=${3:-}
app=target/release/video-editor
[[ -x $app ]] || cargo build --release -p editor_desktop >/dev/null
env ${play:+VE_PROFILE_PLAY=1} "$app" ${project:+"$project"} >/dev/null 2>&1 &
pid=$!
trap 'kill $pid 2>/dev/null || true' EXIT
sleep 6
top -l 13 -s 1 -pid "$pid" -stats pid,cpu,power 2>/dev/null |
  awk -v label="$label" -v pid="$pid" '$1 == pid { n++; if (n > 1) { cpu += $2; pow += $3; k++ } }
       END { if (k) printf "%-40s %6.1f %% CPU   %6.1f energy impact\n", label, cpu / k, pow / k; else print label ": no samples" }'
