#!/usr/bin/env bash
set -euo pipefail
benchmark_root="$1"
platform="$2"
export PATH="$benchmark_root/../tools/bin:$PATH"
export CARGO_BUILD_JOBS=12
export RUSTFLAGS='-C target-cpu=native'
mkdir -p "$benchmark_root/source" "$benchmark_root/artifacts" "$benchmark_root/results"
tar -xf "$benchmark_root/source.tar" -C "$benchmark_root/source"
cd "$benchmark_root/source"
if [[ "$platform" == mac ]]; then
  target=aarch64-apple-darwin
else
  target=x86_64-unknown-linux-gnu
fi
{
  date -u
  rustc -vV
  cargo -V
  cargo pgo --version
  cargo nextest --version
  just --evaluate
  cat Cargo.toml
  cat .cargo/config.toml
} > "$benchmark_root/results/build-environment.txt" 2>&1
cp src/kernels/keccak256.cl "$benchmark_root/final-keccak256.cl"
for variant in baseline final; do
  export CARGO_TARGET_DIR="$benchmark_root/build-$variant"
  [[ ! -e "$CARGO_TARGET_DIR" ]] || { echo 'Expected a clean build directory' >&2; exit 1; }
  cp "$benchmark_root/$variant-keccak256.cl" src/kernels/keccak256.cl
  printf '%s: building fresh %s without PGO\n' "$platform" "$variant"
  cargo build --release --locked --target "$target" --bin salty > "$benchmark_root/results/$variant-build.log" 2>&1
  cp "$CARGO_TARGET_DIR/$target/release/salty" "$benchmark_root/artifacts/$variant"
  printf '%s: training and building fresh %s PGO\n' "$platform" "$variant"
  just pgo-release > "$benchmark_root/results/$variant-pgo.log" 2>&1
  cp "$CARGO_TARGET_DIR/$target/release/salty" "$benchmark_root/artifacts/$variant-pgo"
  cp "$CARGO_TARGET_DIR/pgo-profiles/checked.profdata" "$benchmark_root/artifacts/$variant.profdata"
  printf '%s: validating %s with all native release tests\n' "$platform" "$variant"
  cargo nextest run --locked --release --target "$target" --run-ignored all --test-threads 1 > "$benchmark_root/results/$variant-tests.log" 2>&1
done
printf '%s: both fresh release and PGO builds passed native tests\n' "$platform"
