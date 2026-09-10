#!/usr/bin/env bash
set -euo pipefail
cd /tmp/salty-benchmark-20260910-E6rlwI
benchmark_root="$PWD"
export PATH="$benchmark_root/tools/bin:/home/bryan/.cargo/bin:/home/bryan/.foundry/bin:/usr/local/bin:/usr/bin:/bin"
export CARGO_BUILD_JOBS=12
mkdir -p source artifacts results
tar -xf source.tar -C source
cd source
rustup toolchain install 1.98.0 --profile minimal --component rustfmt,clippy,llvm-tools-preview
cargo install cargo-pgo --locked --root "$benchmark_root/tools"
{
  date --iso-8601=seconds
  hostname
  uname -a
  cat /etc/os-release
  lscpu
  rustc -vV
  cargo -V
  cargo pgo --version
  just --version
  python3 --version
  cc --version
  clinfo
  nvidia-smi -q
  cat /proc/loadavg
} > "$benchmark_root/results/environment.txt" 2>&1
cp src/kernels/keccak256.cl "$benchmark_root/final-keccak256.cl"
export RUSTFLAGS='-C target-cpu=native'
for variant in baseline final; do
  cp "$benchmark_root/$variant-keccak256.cl" src/kernels/keccak256.cl
  export CARGO_TARGET_DIR="$benchmark_root/$variant-build"
  printf 'Building %s without PGO\n' "$variant"
  cargo build --release --locked --target x86_64-unknown-linux-gnu --bin salty > "$benchmark_root/results/$variant-build.log" 2>&1
  cp "$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release/salty" "$benchmark_root/artifacts/$variant"
  printf 'Building and training %s with fresh PGO\n' "$variant"
  just pgo-release > "$benchmark_root/results/$variant-pgo.log" 2>&1
  cp "$CARGO_TARGET_DIR/x86_64-unknown-linux-gnu/release/salty" "$benchmark_root/artifacts/$variant-pgo"
  cp "$CARGO_TARGET_DIR/pgo-profiles/checked.profdata" "$benchmark_root/artifacts/$variant.profdata"
done
sha256sum Cargo.lock src/kernels/keccak256.cl scripts/metal-trials.py "$benchmark_root/artifacts/"* > "$benchmark_root/results/hashes.txt"
"$benchmark_root/artifacts/final-pgo" list > "$benchmark_root/results/devices.txt" 2>&1
printf 'All four builds completed\n'
