# Autoresearch: salty miner throughput

## Objective
Improve OpenCL CREATE2 salt mining throughput for the `salty` binary.

## Metrics
- **Primary**: `attempts_per_sec` (attempts/s, higher is better)

## How to Run
`./autoresearch.sh` outputs `METRIC attempts_per_sec=<number>`.

## Files in Scope
- `src/miner.rs` — OpenCL setup and mining/benchmark loops
- `src/main.rs` — CLI benchmark entrypoint
- `src/kernels/keccak256.cl` — OpenCL kernel

## Off Limits
- Do not change CREATE2 correctness or salt output format.
- Do not remove `mine --once --abi` behavior used by flashprofits.

## Constraints
- `cargo fmt`, `cargo check`, and `cargo clippy -- -D warnings` must pass for kept changes.
- Revert any experiment that does not improve `attempts_per_sec`.

## Termination
Stop after 5 consecutive unsuccessful optimization experiments.

## What's Been Tried
- Baseline harness added: `salty bench` runs warmup + timed OpenCL kernel batches with impossible 21-zero target.

- Experiment 1 discarded: branchless zero-byte count in OpenCL kernel measured 550-559M attempts/s vs 565M baseline.

- Experiment 2 kept: use `uchar` for kernel zero-byte counter. Benchmark improved from 565.2M to ~568.9M attempts/s.
