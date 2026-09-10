# Native NVIDIA OpenCL comparison on ski-lambo-1

This report records the initial regression. The subsequent shared kernel fix
and successful final comparisons on both GPUs are in
[`../cross-gpu-2026-09-10`](../cross-gpu-2026-09-10#final-code-and-fresh-pgo-comparisons).

## Scope and source

Compare the corrected kernel from `8fced24` with the retained kernel at
`305575174838b3008345226540169d2090de2f2a`. Both builds use the same current Rust
source and measurement helper from `c4aab3a9cd8e8f5a71fb986a46e9e1ecfdf141cf`.
Only `src/kernels/keccak256.cl` differs: partial round unrolling and paired
32-bit rotations. The baseline includes the corrected runtime limits and the
current benchmark result readback.

The server has an AMD EPYC 7643 (48 cores, 96 threads), an NVIDIA Tesla T4
(40 compute units, 15 GiB), and Ubuntu 24.04.4 LTS. NVIDIA driver 595.71.05
provides OpenCL 3.0 CUDA 13.2.82 and OpenCL C 1.2. The GPU was idle at initial
inspection, with no allocated device memory. Ordinary server workloads remain
active; this is not a dedicated laboratory host.

## Build and measurement protocol

- Native Rust 1.98.0, LLVM 22.1.8, explicit target
  `x86_64-unknown-linux-gnu`, `RUSTFLAGS='-C target-cpu=native'`.
- Existing release profile: optimization level 3, fat LTO, one codegen unit,
  panic abort, stripped binary, no incremental compilation.
- Separate clean build directories for baseline and final code. Each produces
  a plain release binary, then a fresh profile and optimized binary through
  the unchanged default `just pgo-release` helper. PGO uses real OpenCL mining
  with target 1, minimum 30 seconds, and worksize 71303168.
- Run native release tests, including ignored GPU tests, for both kernels.
  Finish all builds and tests before throughput measurements.
- Use the existing `scripts/metal-trials.py` helper on OpenCL. It uses no
  Metal-specific measurement API. Keep its factory, caller, codehash, and
  worksize 71303168 unchanged.
- For each comparison, use two sets of five alternating baseline/candidate
  pairs. Reverse the starting order in set two. Precondition each set with
  64 warmup batches and 32 measured baseline batches.
- Each paired sample runs eight warmup batches and 32 measured batches:
  2281701376 completed timed hashes. The kernel computes every hash before
  checking the runtime target of 21. Timing includes queue completion and
  normal solution-buffer readback.
- Accept a gain only if both set medians are positive and the stratified
  paired bootstrap 95% interval excludes zero. Use 20000 resamples and seed
  71303168. Preserve every pair.
- Record startup separately, then run real mining with easy target 1 and a
  two-second minimum, and difficult target 21 with a two-second minimum and
  three-second maximum. Check exit codes 0 and 2 and complete 96-byte ABI
  output. Native tests check the full CREATE2 result against the CPU reference.

The isolated remote directory is `/tmp/salty-benchmark-20260910-E6rlwI`.
The source archive contains only Cargo inputs, source, tests, scripts, Justfile,
and toolchain configuration. Build helpers and raw results accompany this report.

## Initial result: the retained Mac code regresses on the T4

Both kernels passed all 31 native release tests, with all ignored GPU tests
included. Both fresh PGO profiles contained 10142 functions and a maximum
function count of 24480. Profile validation and optimization passed without
missing-profile warnings.

| Comparison | Set median paired changes | Pooled median | Paired 95% interval |
| --- | --- | ---: | --- |
| Retained code versus corrected baseline, fresh PGO on both | -16.38%, -16.50% | **-16.45%** | [-16.56%, -16.19%] |
| Retained code versus corrected baseline, no PGO | -15.95%, -16.39% | **-16.16%** | [-16.52%, -15.80%] |
| Partial round unrolling alone versus corrected baseline, no PGO | -1.03%, -0.08% | -0.67% | [-1.08%, -0.10%] |
| Paired 32-bit rotations alone versus corrected baseline, no PGO | -13.00%, -12.95% | **-12.98%** | [-13.12%, -12.70%] |

The initial PGO comparison had separate baseline/final median rates of 659.96
and 551.18 million hashes/s. The non-PGO comparison had rates of 603.84 and
507.11 million hashes/s. These separate run sets do not measure the PGO effect.
Baseline ranges were 9.09% and 5.05% of their medians. Paired gains, not ratios
of those separate medians, determine the reported effect.

Every comparison completed both sets and the easy/difficult mining checks.
Exit codes were 0 and 2, and each result contained the full 96-byte ABI value.
The initial PGO comparison's mining wall times were 2.97/2.40 seconds for
baseline/final at target 1, and 3.90/3.48 seconds at target 21. These single
timing checks establish behavior, not a mining-latency improvement. Startup
estimates were 0.268/0.258 seconds and remain separate from hashing throughput.

The isolated comparisons identify the paired rotations as the main cause of
the NVIDIA regression. The interaction with partial unrolling increases the
combined cost. All isolated variants passed the CPU reference test at nonce
boundaries. The initial retained code is **not faster on this Linux GPU**.
Further kernel work and comparisons on both hosts are recorded in
[`../cross-gpu-2026-09-10`](../cross-gpu-2026-09-10).
