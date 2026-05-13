#!/bin/bash
set -euo pipefail

cargo run --manifest-path /Users/bryan/code/create2-salt-miner/Cargo.toml --release --quiet -- bench \
  --caller 0x0000000000000000000000000000000000000000 \
  --codehash 0x64e604787cbf194841e7b68d7cd28786f6c9a0a3ab9f8b0a0e87cb4387ab0107 \
  --worksize 71303168 \
  --warmup-batches 2 \
  --batches 8
