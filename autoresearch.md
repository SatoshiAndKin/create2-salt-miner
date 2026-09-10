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
- Experiment 3 discarded: `#pragma unroll 20` was slower/noisier at 562-567M attempts/s vs 568.9M best.
- Experiment 4 discarded: early return in zero-byte count was slower at 544-564M attempts/s vs 568.9M best.
- Experiment 5 discarded: doubling benchmark worksize to 142,606,336 yielded 564-565M attempts/s, below 568.9M best.
- Experiment 6 discarded: benchmark target `min_zeros=0` was slower at 555-565M attempts/s vs 568.9M best.

## 2026-09-10: corrected Mac Metal baseline and five trials

The primary target for this run was the Apple M4 Max on macOS 26.2. The runtime
and remote-request fixes were committed as `8fced24` before experiments. Rust
`1.98.0` used LLVM `22.1.8`, `-C target-cpu=native`, and the existing release
profile (fat LTO, one codegen unit, opt-level 3, panic abort). Metal fast math
remained enabled. No PGO was used for the five code trials.

All trials used the same factory, caller, codehash, worksize `71303168`, two
warmup batches, and eight timed batches. The timed count is `570425344` completed
hashes. The benchmark waits for Metal completion and reads the normal solution
buffer. The impossible 21-zero target is a runtime argument. The kernel always
computes Keccak before it checks the target. CPU reference tests also use
match-producing inputs, zero nonces, maximum nonces, and nonce wraparound.

Each trial used five alternating baseline/candidate pairs (AB, BA, AB, BA, AB).
The acceptance gate was a median paired gain greater than the baseline range
as a fraction of its median. A candidate must then pass a second independent
set before retention. None reached the first gate, so none required a
confirmation set. Each set also ran real timed mining with targets 1 and 21,
minimum 2 seconds and maximum 3 seconds. These runs kept normal result handling
and the eight-batch readback schedule. All returned the expected exit code and
96-byte ABI result. Separate native tests checked the full CREATE2 address.

| Trial | Hypothesis and isolated change | Baseline median hashes/s | Candidate median hashes/s | Median paired change | Baseline range | Decision |
| --- | --- | ---: | ---: | ---: | ---: | --- |
| 1 | Move fixed salt and threshold Metal bindings outside the dispatch loop to reduce host calls | 211933604 | 213581132 | +1.81% | 24.73% | Reject: below variation |
| 2 | Pack the three Metal dispatch arguments into one block to reduce argument-setting calls | 215857564 | 216461254 | +0.28% | 2.36% | Reject: below variation |
| 3 | Use 128 threads per group, within device limits, to test scheduling with the large Keccak state | 206582107 | 219846333 | +5.61% | 18.39% | Reject: below variation |
| 4 | Initialize Keccak with packed 64-bit words to reduce byte writes | 217391946 | 224244660 | +2.74% | 29.25% | Reject: below variation |
| 5 | Keep only two old lane values during each chi row to reduce temporary storage | 215566458 | 219665717 | -0.06% | 8.81% | Reject: no paired gain |

The initial identical-binary baseline set had a 52.47% range in its baseline
samples. Concurrent CPU builds and ordinary desktop activity limited precision.
This run demonstrates no retained throughput gain. It does not prove that the
small changes can never help on a quieter machine. The CPU Time Profiler trace
recorded 27 active samples, mainly at startup and shutdown, and no active sample
during the long GPU wait. Metal System Trace recorded compute work for the
benchmark process. This supported testing shader scheduling and storage after
the two host-call trials.

Five consecutive trials failed, so the run stopped under the agreed rule.
Partial unrolling, paired 32-bit rotations, and solution atomic contention were
not tested. No experimental code remains in the active mining paths. The
combined retained code gain against the corrected baseline is **0%**.

Raw pairs, timed mining output, separate startup measurements, compiler/device
metadata, binary SHA-256 values, and rejected patches are in
[`experiments/metal-2026-09-10`](experiments/metal-2026-09-10). The measurement
script is [`scripts/metal-trials.py`](scripts/metal-trials.py) and requires Python
3.10 or newer. An initial script attempt failed under system Python 3.9 and also
overlapped a GPU test; it is excluded. Recorded sets used Python 3.14. No
benchmark changes or repeated measurements count as code trials.

Linux ARM64 PoCL passed locked check, strict Clippy, 28 CPU tests, native timed
mining, and fresh default `just pgo-release` training/merge/optimization. The
profile contained 10115 functions with a maximum function count of 1275. The
first Linux test build failed to load a dependency artifact; a fresh build
directory passed. This is OpenCL runtime evidence on PoCL, not a Linux GPU or
Windows performance measurement. The rejected shared kernel changes have no
OpenCL runtime validation and were not retained.
