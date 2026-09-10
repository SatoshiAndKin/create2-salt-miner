#!/usr/bin/env bash
set -euo pipefail
benchmark_root="$1"
platform="$2"
if [[ "$platform" == mac ]]; then
  python=/opt/homebrew/bin/python3.14
else
  python=python3
fi
cd "$benchmark_root/source"
printf '%s: measuring matching addresses with fresh PGO builds\n' "$platform"
"$python" scripts/metal-trials.py "$benchmark_root/artifacts/baseline-pgo" "$benchmark_root/artifacts/final-pgo" "$benchmark_root/results/final-matching-pgo.json" --zeros 1
printf '%s: measuring PGO effect with two sets of ten pairs\n' "$platform"
"$python" scripts/metal-trials.py "$benchmark_root/artifacts/final" "$benchmark_root/artifacts/final-pgo" "$benchmark_root/results/final-pgo-effect.json" --pairs 10
if [[ "$platform" != mac ]]; then
  { date -u; nvidia-smi -q; cat /proc/loadavg; } > "$benchmark_root/results/device-after.txt" 2>&1
fi
printf '%s: all final measurements completed\n' "$platform"
