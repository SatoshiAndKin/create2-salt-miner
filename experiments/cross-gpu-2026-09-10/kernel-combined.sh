#!/usr/bin/env bash
set -euo pipefail
benchmark_root="$1"
platform="$2"
export PATH="$benchmark_root/tools/bin:$PATH"
export CARGO_BUILD_JOBS=12
export RUSTFLAGS='-C target-cpu=native'
if [[ "$platform" == mac ]]; then
  target=aarch64-apple-darwin
  export CARGO_TARGET_DIR="$benchmark_root/build"
  python=/opt/homebrew/bin/python3.14
  reference=/Users/bryan/code/create2-salt-miner/target/metal-final/baseline
  cp "$benchmark_root/metal.rs" "$benchmark_root/source/src/miner/metal.rs"
else
  target=x86_64-unknown-linux-gnu
  export CARGO_TARGET_DIR="$benchmark_root/final-build"
  python=python3
  reference="$benchmark_root/artifacts/baseline"
fi
cd "$benchmark_root/source"
for variant in union-packed; do
  cp "$benchmark_root/$variant-keccak256.cl" src/kernels/keccak256.cl
  printf 'Building %s on %s\n' "$variant" "$platform"
  cargo build --release --locked --target "$target" --bin salty > "$benchmark_root/results/$variant-build.log" 2>&1
  cp "$CARGO_TARGET_DIR/$target/release/salty" "$benchmark_root/artifacts/$variant"
  if [[ "$platform" == mac ]]; then
    cargo nextest run --locked --release --target "$target" --run-ignored all --test-threads 1 > "$benchmark_root/results/$variant-tests.log" 2>&1
  else
    cargo test --release --locked --target "$target" -- --include-ignored --test-threads=1 > "$benchmark_root/results/$variant-tests.log" 2>&1
  fi
done
if [[ "$platform" != mac ]]; then
  mkdir -p examples
  cp "$benchmark_root/opencl-inspect.rs" examples/opencl-inspect.rs
  cargo run --release --locked --target "$target" --example opencl-inspect -- "$benchmark_root/"*-keccak256.cl > "$benchmark_root/results/compiler-resources-combined.log" 2>&1
fi
printf 'All candidates built and passed native tests on %s\n' "$platform"
for variant in union-packed; do
  printf 'Measuring %s on %s\n' "$variant" "$platform"
  "$python" scripts/metal-trials.py "$reference" "$benchmark_root/artifacts/$variant" "$benchmark_root/results/$variant.json"
done
printf 'All kernel candidate comparisons completed on %s\n' "$platform"
