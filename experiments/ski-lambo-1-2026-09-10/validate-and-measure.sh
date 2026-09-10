#!/usr/bin/env bash
set -euo pipefail
cd /tmp/salty-benchmark-20260910-E6rlwI
benchmark_root="$PWD"
export PATH="$benchmark_root/tools/bin:/home/bryan/.cargo/bin:/home/bryan/.foundry/bin:/usr/local/bin:/usr/bin:/bin"
export CARGO_BUILD_JOBS=12
export RUSTFLAGS='-C target-cpu=native'
cd source
for variant in baseline final; do
  cp "$benchmark_root/$variant-keccak256.cl" src/kernels/keccak256.cl
  export CARGO_TARGET_DIR="$benchmark_root/$variant-build"
  printf 'Testing %s on the native OpenCL GPU\n' "$variant"
  cargo test --release --locked --target x86_64-unknown-linux-gnu -- --include-ignored --test-threads=1 > "$benchmark_root/results/$variant-tests.log" 2>&1
done
printf 'Both native test suites passed\n'
for mode in pgo code; do
  baseline="$benchmark_root/artifacts/baseline"
  candidate="$benchmark_root/artifacts/final"
  if [[ "$mode" == pgo ]]; then
    baseline="$baseline-pgo"
    candidate="$candidate-pgo"
  fi
  {
    date --iso-8601=seconds
    nvidia-smi -q
    cat /proc/loadavg
  } > "$benchmark_root/results/$mode-before.txt"
  printf 'Measuring combined %s gain with two sets of five alternating pairs\n' "$mode"
  python3 scripts/metal-trials.py "$baseline" "$candidate" "$benchmark_root/results/combined-$mode.json" | tee "$benchmark_root/results/combined-$mode-output.log"
  {
    date --iso-8601=seconds
    nvidia-smi -q
    cat /proc/loadavg
  } > "$benchmark_root/results/$mode-after.txt"
done
printf 'All native tests and paired measurements completed\n'
