#!/usr/bin/env bash
# Clone every third-party source at its pinned ref into third_party/.
# Pins live in third_party/PINS; bump a pin there, then rerun.
set -euo pipefail
cd "$(dirname "$0")/../third_party"

while read -r name url ref; do
  [[ -z "$name" || "$name" == \#* ]] && continue
  if [[ -d "$name/.git" ]]; then
    git -C "$name" fetch -q --depth 1 origin "$ref"
    git -C "$name" checkout -q FETCH_HEAD
  else
    git clone -q --depth 1 --branch "$ref" "$url" "$name" 2>/dev/null \
      || { git init -q "$name"; git -C "$name" remote add origin "$url";
           git -C "$name" fetch -q --depth 1 origin "$ref"; git -C "$name" checkout -q FETCH_HEAD; }
  fi
  git -C "$name" submodule update -q --init --recursive --depth 1 || true
  echo "$name $(git -C "$name" rev-parse --short HEAD)"
done < PINS
