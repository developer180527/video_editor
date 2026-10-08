#!/usr/bin/env bash
# Enforces the architecture's dependency rules. Run in CI; exits non-zero on
# any violation.
#
#  1. Core crates never branch on the OS: no cfg(target_os / windows / unix).
#  2. Core crates never depend on adapters, the shell, the UI, or libgui.
#  3. The UI depends on the engine API, not on engine internals or adapters.
#  4. Adapters never depend on each other's OS crates except the portable
#     headless baseline.
set -uo pipefail
cd "$(dirname "$0")/.."
fail=0
core=(ve_time ve_model ve_command ve_ports ve_render ve_playback ve_plugin_abi ve_plugin_host ve_engine)

deps() { # direct normal dependencies of a crate
  cargo metadata --format-version 1 --no-deps 2>/dev/null |
    python3 -c "import json,sys; m=json.load(sys.stdin)
for p in m['packages']:
    if p['name']=='$1':
        print('\n'.join(d['name'] for d in p['dependencies'] if d['kind'] is None))"
}

for c in "${core[@]}"; do
  if grep -rnE '(#!?\[cfg|cfg!)\(.*(target_os|windows|unix|target_family)' "crates/$c/src" >/dev/null; then
    echo "✗ $c uses an OS cfg:"; grep -rnE '(#!?\[cfg|cfg!)\(.*(target_os|windows|unix|target_family)' "crates/$c/src"; fail=1
  fi
  for d in $(deps "$c"); do
    case "$d" in
      platform_*|media_*|audio_*|ve_ui|ve_builtins|libgui*|winit)
        echo "✗ core crate $c depends on $d"; fail=1 ;;
    esac
  done
done

for d in $(deps ve_ui); do
  case "$d" in
    platform_*|media_*|audio_*|ve_command|ve_plugin_host|ve_playback|ve_media|winit)
      echo "✗ ve_ui depends on $d (talk to ve_engine instead)"; fail=1 ;;
  esac
done

for a in platform_desktop platform_ios platform_headless; do
  for d in $(deps "$a"); do
    case "$d" in
      platform_headless) [[ "$a" == platform_headless ]] && { echo "✗ $a depends on itself"; fail=1; } ;;
      platform_*) echo "✗ adapter $a depends on adapter $d"; fail=1 ;;
    esac
  done
done

[[ $fail == 0 ]] && echo "✓ architecture rules hold"
exit $fail
