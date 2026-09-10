#!/usr/bin/env bash
set -euo pipefail
cd /tmp/salty-benchmark-20260910-E6rlwI
benchmark_root="$PWD"
export PATH="$benchmark_root/tools/bin:/home/bryan/.cargo/bin:/home/bryan/.foundry/bin:/usr/local/bin:/usr/bin:/bin"
export CARGO_BUILD_JOBS=12
export RUSTFLAGS='-C target-cpu=native'
export CARGO_TARGET_DIR="$benchmark_root/final-build"
cd source
trap 'cp "$benchmark_root/final-keccak256.cl" src/kernels/keccak256.cl' EXIT
for variant in partial-only rotation-only; do
  cp "$benchmark_root/$variant-keccak256.cl" src/kernels/keccak256.cl
  printf 'Building isolated %s diagnostic\n' "$variant"
  cargo build --release --locked --target x86_64-unknown-linux-gnu --bin salty > "$benchmark_root/results/$variant-build.log" 2>&1
  cp "$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release/salty" "$benchmark_root/artifacts/$variant"
  cargo test --release --locked --target x86_64-unknown-linux-gnu opencl_scores_match_cpu_at_nonce_boundaries -- --include-ignored --test-threads=1 > "$benchmark_root/results/$variant-tests.log" 2>&1
done
cp "$benchmark_root/final-keccak256.cl" src/kernels/keccak256.cl
printf 'Both isolated variants built and passed CPU reference checks\n'
for variant in partial-only rotation-only; do
  printf 'Measuring isolated %s effect\n' "$variant"
  python3 scripts/metal-trials.py "$benchmark_root/artifacts/baseline" "$benchmark_root/artifacts/$variant" "$benchmark_root/results/$variant.json" | tee "$benchmark_root/results/$variant-output.log"
done
sha256sum "$benchmark_root/artifacts/"* "$benchmark_root/"*-keccak256.cl > "$benchmark_root/results/diagnostic-hashes.txt"
printf 'Isolated diagnostic comparisons completed\n'
