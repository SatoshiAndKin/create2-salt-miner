#!/usr/bin/env bash
set -euo pipefail
benchmark_root="$1"
platform="$2"
if [[ "$platform" == mac ]]; then
  python=/opt/homebrew/bin/python3.14
  {
    date -u
    sw_vers
    system_profiler SPDisplaysDataType
    pmset -g custom
  } > "$benchmark_root/results/device-before.txt" 2>&1
else
  python=python3
  {
    date -u
    nvidia-smi -q
    cat /proc/loadavg
  } > "$benchmark_root/results/device-before.txt" 2>&1
fi
cd "$benchmark_root/source"
for mode in pgo code; do
  baseline="$benchmark_root/artifacts/baseline"
  final="$benchmark_root/artifacts/final"
  if [[ "$mode" == pgo ]]; then
    baseline="$baseline-pgo"
    final="$final-pgo"
  fi
  printf '%s: measuring final %s comparison\n' "$platform" "$mode"
  "$python" scripts/metal-trials.py "$baseline" "$final" "$benchmark_root/results/final-$mode.json"
done
printf '%s: primary final measurements completed\n' "$platform"
