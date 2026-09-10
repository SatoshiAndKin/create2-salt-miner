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

Mac `just pgo-release` also completed with fresh default training. The profile
contained 8302 functions and a maximum function count of 3570. Optimization
reported 53 functions without training data. The five-pair PGO comparison had a
median paired change of -9.47% against a 26.84% baseline range, so this run
establishes no PGO throughput gain or regression. PGO remains required by the
release policy. The raw comparison is `pgo-set1.json`. There are no retained code
optimizations to combine or retrain separately; the corrected and final mining
code are the same apart from tests. No total speedup is claimed.

Final local Mac validation passed 32 tests, including all native device tests,
locked check, strict Clippy, formatting, and cargo-deny. The Python metadata
checks and PowerShell syntax check passed. Native Windows profile training and
the resulting Windows profile-guided release build remain required before this
release repair is complete.

Windows GNU locked cross-check passed both locally and in Linux x86-64 CI.
Instrumentation then failed with E0463: the stock Rust 1.98.0 GNU target lacks
`profiler_builtins`. The [Rust release build configuration](https://github.com/rust-lang/rust/blob/1.98.0/src/ci/github-actions/jobs.yml#L659-L738)
enables the runtime for MSVC and GNU LLVM, but not this GNU target. A target or
compiler-build decision is required. No Windows profile has been fabricated,
and the mandatory profile checks still block the release.

The first Linux x86-64 CI run also exposed a timing assumption in the native CLI
test. Slow PoCL batches reached the maximum before collecting a target score,
so the specified fallback exit code 2 was correct. The qualification test now
uses a minimum without a competing maximum. The difficult-target test still
uses a maximum shorter than its minimum and checks fallback output against CPU
CREATE2. All 32 native Mac tests passed after this test correction.

Both backends use `std::time::Instant`: the import in `miner.rs` serves OpenCL,
and `miner/metal.rs` has its own import. Both pass elapsed `Duration` values to
the same platform-independent `MiningStop::reached` function. The conditional
import prevents an unused-import warning on macOS; it does not disable timing.

The [final Linux x86-64 CI run](https://github.com/SatoshiAndKin/create2-salt-miner/actions/runs/34457637380/job/102807649578)
passed all 30 tests, including native PoCL CLI and queued mining, formatting,
locked check, strict Clippy, cargo-deny, and Python metadata tests. Fresh default
PGO training produced 10220 functions with a maximum function count of 1020;
profile validation and the optimized build passed. CI uploaded the Linux binary
and profiles. Windows cross-check passed again in that run; instrumentation
remains blocked by the missing GNU profiling runtime.

## 2026-09-10: marginal candidates under the revised gain gate

The user replaced the baseline-range gate with paired confidence testing. Retry
threadgroup size and packed state first, then revisit other marginal ideas.
Use two separate sets of five alternating pairs, reversing the starting order
in set two. Each sample uses eight warmup batches and 32 timed batches, or
2281701376 completed hashes. Each set starts with 64 warmup batches and a
32-batch baseline measurement. Keep factory, caller, codehash, and worksize
unchanged. Run no builds or other mining commands during measurements.

Accept only when both set medians are positive and the pooled paired 95%
bootstrap interval excludes zero. Use 20000 percentile bootstrap resamples,
resampling pairs within each set, with fixed seed 71303168. Record every sample,
including outliers, startup, easy/difficult timed mining, and raw ABI output.
This is an estimate from these measurements, not a performance claim for other
hardware. Keep the mining correctness and timeout checks. Confirmation runs and
retries do not count as new distinct optimization ideas.

The first control used the same corrected binary for both labels. Its set
medians were -0.04% and -1.98%; the pooled interval was [-4.08%, +0.20%]. It did
not pass the gain gate. Its throughput data is retained in `control.json`, but
the subsequent easy mining check exposed a harness assumption: a three-second
maximum can correctly return a score-zero fallback. The helper now separates
minimum-only qualification from difficult-target maximum expiry, as the native
CLI test does. A complete control rerun follows this correction. This is a
harness repair, not an optimization trial.

The complete control rerun passed its mining checks. Its set medians were
-7.96% and +7.02%, with pooled interval [-8.77%, +9.24%]. The Mac ran on AC power
with low-power mode disabled. Background system load and desktop activity still
limit precision. The device reported execution width 32, maximum threads 1024,
and an actual baseline group size of 256.

| Retry | Set medians | Pooled paired 95% interval | Decision |
| --- | --- | --- | --- |
| 128-thread groups, five pairs per set | +7.57%, +7.52% | [-1.33%, +17.36%] | Larger independent confirmation warranted |
| 128-thread groups, ten fresh pairs per set | -0.51%, -2.64% | [-4.91%, +1.18%] | Do not retain: gain did not repeat |
| Packed state, five pairs per set | -10.05%, +3.43% | [-6.55%, +6.40%] | Do not retain: sets disagree and interval includes zero |
| Invariant bindings, five pairs per set | -2.33%, +0.31% | [-5.32%, +0.11%] | Do not retain: sets disagree and interval includes zero |
| Partial round unrolling, five pairs per set | +38.13%, +47.85% | [+41.33%, +55.39%] | Retain: both sets pass, with native correctness checks |

Packed state passed 31 native Linux PoCL tests, including CPU-reference checks
at nonce byte/word boundaries and match-producing inputs. The larger threadgroup run uses fresh samples
without pooling the earlier selection data. Further confirmation, if warranted
by positive medians with an interval crossing zero, is limited to one larger
fresh run per candidate; the gain gate itself stays unchanged.

Partial round unrolling is the sixth distinct code idea in this run. It replaces
23 expanded Keccak rounds with a constant table and a loop with `#pragma unroll 4`.
The existing final partial round stays intact. All 32 native Metal tests and
31 native Linux PoCL tests passed, including CPU-reference checks for nonce
boundaries, matching inputs, timed mining, and remote requests. Linux strict
Clippy passed. This change reduces shader expansion; the earlier CPU profile
placed the measured work on the GPU. The pooled paired median gain was 46.88%.
The root now retains this kernel. Later trials must compare against it.

The matching baseline and candidate were built in the same scratch source and
target directory with stable 1.98.0, explicit `-C target-cpu=native`, and unchanged
release settings. `partial-rounds.json` records both binary hashes, all commands,
startup, and timed mining. `partial-rounds.patch` records the isolated change.
Final combined and fresh PGO comparisons remain to be completed.

## Nightly Rust feature assessment added to the plan

Assess nightly features against the measured work before changing source. Keep
stable 1.98.0 as the release contract during the assessment. A nightly compiler
comparison must use a dated toolchain, the same kernel and release options, fresh
profiles from that compiler, and the same paired gain gate. Report compiler,
code, and PGO effects separately. Check Mac, Linux, and Windows support before
proposing a release toolchain change.

| Feature or check | Fit to this miner and next action |
| --- | --- |
| [Explicit tail calls (`become`)](https://doc.rust-lang.org/stable/core/keyword.become.html) | Inspect hot recursive calls or interpreter dispatch. The current host mining path uses loops and waits for GPU work; no measured tail-call opportunity exists. Rust marks this feature incomplete. Do not rewrite the loop into recursion to force a trial. |
| [Portable SIMD](https://doc.rust-lang.org/nightly/std/simd/index.html) | Look for repeated host data processing. Keccak executes in Metal/OpenCL source, so Rust SIMD does not change the hashing kernel. CPU reference hashing checks results and is outside the throughput bottleneck. |
| [Branch hints](https://doc.rust-lang.org/nightly/std/hint/fn.likely.html) | Check CPU branch samples before adding hints to result handling. Mandatory PGO already measures host branch frequency. Current CPU evidence does not justify a separate hint trial. |
| [MIR optimization levels](https://doc.rust-lang.org/nightly/unstable-book/compiler-flags/mir-opt-level.html) and newer LLVM | Compare a dated nightly compiler only if the host profile identifies a material cost after the kernel gain. The internal MIR option affects Rust code, not the runtime Metal/OpenCL compiler. Use optimization remarks to identify a concrete missed optimization first. |
| [Optimization remarks](https://doc.rust-lang.org/rustc/codegen-options/#remark) | Available on stable. Use `-C remark` for an identified Rust hot function; inspect generated code alongside CPU samples. |
| [Sample-based PGO](https://doc.rust-lang.org/beta/unstable-book/compiler-flags/profile_sample_use.html) | Optional investigation for a supported native host with representative CPU samples. Keep the required instrumented PGO release path. Never reuse profiles across compiler versions. |
| [Compiler self-profile](https://doc.rust-lang.org/unstable-book/compiler-flags/self-profile.html) | Measures compilation, not miner throughput. Use only to investigate build time and report that metric separately. |

The first feature pass found no supported reason to add nightly syntax to the
current GPU-bound path. Recheck CPU cost after the retained kernel changes. This
screening is research and does not count as an optimization trial.
