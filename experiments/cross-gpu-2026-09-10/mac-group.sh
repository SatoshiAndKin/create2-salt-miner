#!/usr/bin/env bash
set -euo pipefail
cd /Users/bryan/code/create2-salt-miner
benchmark_root="$PWD/target/cross-gpu"
mkdir -p "$benchmark_root/source" "$benchmark_root/artifacts" "$benchmark_root/results"
tar -xf target/ski-lambo-1/source.tar -C "$benchmark_root/source"
cp "$benchmark_root/source/src/miner/metal.rs" "$benchmark_root/metal.rs"
sed 's/maximum_threads.min(256)/maximum_threads/' "$benchmark_root/metal.rs" > "$benchmark_root/source/src/miner/metal.rs"
cd "$benchmark_root/source"
export CARGO_TARGET_DIR="$benchmark_root/build"
export RUSTFLAGS='-C target-cpu=native'
printf 'Building Metal with the device-reported maximum threadgroup size\n'
cargo build --release --locked --target aarch64-apple-darwin --bin salty > "$benchmark_root/results/device-group-build.log" 2>&1
cp "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/salty" "$benchmark_root/artifacts/device-group"
printf 'Checking native Metal correctness\n'
cargo nextest run --locked --release --target aarch64-apple-darwin --run-ignored all --test-threads 1 > "$benchmark_root/results/device-group-tests.log" 2>&1
printf 'Measuring the device-derived Metal group size\n'
/opt/homebrew/bin/python3.14 scripts/metal-trials.py /Users/bryan/code/create2-salt-miner/target/metal-final/final "$benchmark_root/artifacts/device-group" "$benchmark_root/results/device-group.json"
printf 'Metal device-group comparison completed\n'
