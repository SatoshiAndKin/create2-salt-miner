# Autoresearch: salty miner throughput

## Objective
Improve CREATE2 salt mining throughput for the `salty` binary. Mac Metal is the
primary target for the current run. Validate shared kernel changes on OpenCL.

## Metrics
- **Primary**: `attempts_per_sec` (attempts/s, higher is better)

## How to Run
`./autoresearch.sh` outputs `METRIC attempts_per_sec=<number>`.

## Files in Scope
- `src/miner.rs` — OpenCL setup and mining/benchmark loops
- `src/miner/metal.rs` — Metal setup and mining/benchmark loops
- `src/main.rs` — CLI benchmark entrypoint
- `src/kernels/keccak256.cl` — shared Metal/OpenCL kernel
- `scripts/metal-trials.py` — paired measurement protocol

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
not tested in that first run. At that point, no experimental code remained in
the active mining paths, and the combined retained code gain was **0%**.

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
Windows performance measurement. At that stage, the rejected shared kernel
changes had no OpenCL runtime validation and were not retained.

Mac `just pgo-release` also completed with fresh default training. The profile
contained 8302 functions and a maximum function count of 3570. Optimization
reported 53 functions without training data. The five-pair PGO comparison had a
median paired change of -9.47% against a 26.84% baseline range, so this run
establishes no PGO throughput gain or regression. PGO remains required by the
release policy. The raw comparison is `pgo-set1.json`. That first run retained no
code optimizations to combine or retrain separately; its corrected and final
mining code were the same apart from tests. No total speedup was claimed then.

First-run local Mac validation passed 32 tests, including all native device tests,
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

The [first-run Linux x86-64 CI run](https://github.com/SatoshiAndKin/create2-salt-miner/actions/runs/34457637380/job/102807649578)
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
| Chi storage after partial unrolling, five pairs per set | +1.42%, +0.04% | [-2.19%, +3.94%] | Larger independent confirmation warranted |
| Chi storage after partial unrolling, ten fresh pairs per set | +1.27%, -0.93% | [-2.22%, +3.28%] | Do not retain: the small gain did not repeat |
| Paired 32-bit rotations after partial unrolling, five pairs per set | +7.77%, +7.43% | [+4.49%, +11.17%] | Retain: both sets and correctness checks pass |
| Atomic guard, target 1, five pairs per set | -1.27%, -0.91% | [-2.50%, +1.30%] | No demonstrated gain with frequent matches |
| Atomic guard, target 21, five pairs per set | +1.30%, -7.13% | [-4.16%, +4.01%] | No clear hashing throughput change |
| Atomic guard, target 0, five pairs per set | +2.78%, +3.36% | [-0.20%, +5.68%] | Larger independent fallback-workload confirmation warranted |
| Atomic guard, target 0, ten fresh pairs per set | +1.87%, -2.03% | [-4.70%, +5.06%] | Do not retain: the fallback gain did not repeat |

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

The chi retry also passed all 32 native Metal tests and 31 Linux PoCL tests,
including the CPU-reference boundary workloads. Both measured binaries contain
the retained partial unrolling; the candidate changes only the chi row's
temporary values. The confirmation used the unchanged script snapshot from
`6a02e93`, before the new benchmark target option. All fresh confirmation samples
remain in `chi-after-partial-confirm.json`; earlier selection samples are not
pooled with them.

Paired rotations are the seventh distinct code idea. The candidate implements
the non-AMD Keccak rotations with paired 32-bit shifts and preserves the existing
AMD `bitalign` implementation. All 32 native Metal tests and 31 Linux PoCL tests
passed. Both measured binaries contain partial unrolling and use the benchmark
before the target-option change. The pooled median gain was 7.60% over partial
unrolling. This is an incremental gain; measure the combined gain directly.
The CPU-reference tests now also require a known input with at least two zero
bytes for each salt tail, which strengthens checks against an incorrect digest.

The atomic guard is the eighth distinct code idea. It reads the Metal winner
flag after hashing and target qualification, then skips redundant atomic
exchanges once a writer has won. The exchange still selects exactly one writer,
and command completion still precedes host readback. The candidate passed all
32 native Metal tests. Its OpenCL kernel is unchanged from the retained kernel
that passed 31 Linux PoCL tests. Targets 1, 21, and 0 test frequent matches, no
matches, and the initial fallback threshold respectively. Each uses the same
completed hash count and explicit target. These workloads are one optimization
trial, not three separate ideas. A gain at target 0 must not be reported as a
steady hashing gain. The larger confirmation does not pool its selection data.

All eight planned distinct code ideas have now been tested. Retain partial
unrolling and paired rotations. Leave invariant bindings, packed arguments,
128-thread groups, packed state initialization, reduced chi storage, and the
atomic guard out of the final code. The revised gate accepted the repeatable
7.60% rotation gain; marginal positive selection results received fresh larger
confirmations when warranted, but those gains did not repeat.

The [Linux x86-64 CI run for the retained changes](https://github.com/SatoshiAndKin/create2-salt-miner/actions/runs/34465931839/job/102834392121)
passed validation and fresh default PGO training/optimization. The Windows
cross-check passed in the same run; the instrumented build still fails because
stock Rust 1.98.0 GNU lacks `profiler_builtins`.
The Linux run passed all 31 native PoCL tests and its profile contained 10245
functions with a maximum function count of 1020.

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
| [Branch hints](https://doc.rust-lang.org/nightly/std/hint/fn.likely.html) | Check CPU branch samples before adding hints to result handling. Mandatory PGO already measures host branch frequency. The related [`cold_path`](https://doc.rust-lang.org/nightly/std/hint/fn.cold_path.html) is stable since 1.95. Current CPU evidence does not justify a separate hint trial. |
| [MIR optimization levels](https://doc.rust-lang.org/nightly/unstable-book/compiler-flags/mir-opt-level.html) and newer LLVM | Compare a dated nightly compiler only if the host profile identifies a material cost after the kernel gain. The internal MIR option affects Rust code, not the runtime Metal/OpenCL compiler. Use optimization remarks to identify a concrete missed optimization first. |
| [Optimization remarks](https://doc.rust-lang.org/rustc/codegen-options/#remark) | Available on stable. Use `-C remark` for an identified Rust hot function; inspect generated code alongside CPU samples. |
| [Sample-based PGO](https://doc.rust-lang.org/beta/unstable-book/compiler-flags/profile_sample_use.html) | Optional investigation for a supported native host with representative CPU samples. Keep the required instrumented PGO release path. Never reuse profiles across compiler versions. |
| [Compiler self-profile](https://doc.rust-lang.org/unstable-book/compiler-flags/self-profile.html) | Measures compilation, not miner throughput. Use only to investigate build time and report that metric separately. |

The first feature pass found no supported reason to add nightly syntax to the
current GPU-bound path. Recheck CPU cost after the retained kernel changes. This
screening is research and does not count as an optimization trial.
The installed dated nightly is `nightly-2026-09-10`, rustc `a36d05efa` (1.100.0),
with LLVM 23.1.1. Its availability does not establish a runtime gain.

After both retained kernel changes, `/usr/bin/time -lp` reported 0.00 seconds of
user CPU time and 0.01 seconds of system CPU time over 23.62 seconds of wall time
for 64 warmup batches and 128 timed batches. These CPU values have 0.01-second
printed precision. `host-cpu.json` records the exact command, binary hash, metric,
and resource report. This agrees with the earlier CPU Time Profiler evidence:
the measured mining workload spends its time on the GPU. No nightly host-code
experiment is justified by these samples. No compiler gain is claimed, and the
stable release pin remains 1.98.0.

## Mac combined measurements before Linux GPU verification

These historical results cover the first two retained kernel changes. The
cross-GPU revision below supersedes them for the current code.

The final code retains partial round unrolling and paired 32-bit rotations.
The corrected baseline uses the kernel from `8fced24` with the same current
benchmark and Rust source as the final build. Both use explicit target
`aarch64-apple-darwin`, stable Rust 1.98.0, `-C target-cpu=native`, and the same
release settings. Separate clean build directories prevent profile/artifact
reuse. Each ran the unchanged default `just pgo-release` sequence, including
worksize 71303168, target 1, and minimum training time 30 seconds. Both profiles
contained 8304 functions with maximum function count 3570. Each optimized build
reported 53 functions without profile data. Profile validation and both builds
passed.

All final comparisons use two sets of five alternating pairs, except the fresh
PGO-effect confirmation, which uses ten pairs per set. Each sample completes
2281701376 timed hashes after eight warmup batches. No local build or other
agent-started mining command overlapped measurements. Desktop activity and
background load still caused substantial variation.

| Comparison | Set median paired changes | Pooled paired median | Paired 95% interval |
| --- | --- | ---: | --- |
| Combined code, without PGO | +48.96%, +70.09% | **+60.71%** | [+55.01%, +65.22%] |
| Combined code, both builds with fresh PGO | +52.56%, +75.96% | **+62.40%** | [+54.92%, +79.61%] |
| PGO effect on final code, initial comparison | -10.96%, -2.76% | -6.22% | [-8.75%, -0.67%] |
| PGO effect on final code, fresh larger confirmation | -0.10%, -1.00% | -0.29% | [-1.87%, +1.91%] |

The combined gain passes the revised gate with and without PGO. The initial PGO
slowdown did not repeat in the larger confirmation. Both final binaries contain
the exact retained kernel bytes. These results establish no repeatable PGO
throughput gain or slowdown; PGO remains mandatory. Do not subtract the two
combined code percentages to infer a PGO gain: they are separate run sets.

In the PGO code comparison, the baseline and final median rates were 271.6 and
432.9 million hashes/s. The baseline range was 36.70% of its median. In the
non-PGO comparison, the rates were 299.9 and 445.3 million hashes/s, with a
51.40% baseline range. Reported gains use paired ratios, not the ratio of these
separate rate medians. These are results for this Mac and workload.

Every completed final comparison passed easy minimum-only mining and difficult
maximum-limited fallback checks, with exit codes 0 and 2 and complete 96-byte
ABI output. The PGO comparison's easy mining wall times were 3.54 seconds for the
baseline and 2.34 seconds for the final build; difficult mining took 3.78 and
3.44 seconds. These single timing checks establish behavior, not a separate
latency speedup. Limits still apply at batch boundaries. Startup estimates are
stored separately; no startup improvement is claimed.

Raw pairs, mining outputs, startup estimates, compiler/flag/source fingerprints,
profile hashes, and build/training logs are in
[`experiments/metal-2026-09-10-final`](experiments/metal-2026-09-10-final).
The native Mac checks passed all 32 tests, including device tests. Linux ARM64
PoCL and Linux x86-64 CI passed 31 tests. Formatting, locked check, strict Clippy,
Python checks, cargo-deny, and Windows GNU cross-check passed. Cargo-deny reports
duplicate versions and the existing yanked `chacha20 0.10.1` dependency.
This is Metal runtime and PoCL correctness
evidence; Windows performance remains unmeasured.

The Windows release repair remains incomplete: stock Rust 1.98.0 GNU cannot
build the instrumented executable without `profiler_builtins`. A supported
target or compiler build, native Windows training, a committed fresh profile,
and a successful profile-guided Windows release build are still required.

## 2026-09-10: shared kernel improvements on Mac and ski-lambo-1

Native testing on `ski-lambo-1` found a real regression in the first Mac-retained
kernel: **-16.45%** with fresh PGO on the Tesla T4, with paired 95% interval
[-16.56%, -16.19%]. Isolated tests identified paired rotations as the main cost.
The initial negative results remain in
[`experiments/ski-lambo-1-2026-09-10`](experiments/ski-lambo-1-2026-09-10).

The device audit found no worksize-dependent allocation based on Mac memory.
Worksize 71303168 counts hashes per dispatch. Metal queries the pipeline's
execution width and maximum group size, then aligns and caps its preference
of 256 threads. OpenCL lets the driver select the local group. Removing the
Metal preference and using its detected maximum produced -0.23%, with interval
[-1.23%, +1.06%], so that trial was rejected. A maximum supported size did not
improve this kernel's measured throughput.

Commit `76f24fe` removes the fixed round-loop unroll hint and uses the existing
union to express paired rotations. It also retains packed state initialization,
which improved T4 throughput in its isolated retry. These are retries within
the earlier ideas. NVIDIA PTX showed a further opportunity in address scoring:
twenty byte tests and additions. The ninth idea replaces these with exact
zero-byte masks and popcounts over five words. Its incremental T4 gain is
0.94%, with interval [+0.69%, +1.34%]; Mac shows no repeatable change.
The final shared kernel adds no vendor/model checks or tuning flags.

Nine distinct ideas have now been tested. Four remain: the round loop with
compiler-selected unrolling, union-based paired rotations, packed state, and
word scoring. Five remain rejected: invariant bindings, packed arguments,
threadgroup tuning, reduced chi storage, and the atomic guard. The complete
retry table, patches, compiler resources, and raw data are in
[`experiments/cross-gpu-2026-09-10`](experiments/cross-gpu-2026-09-10).

All final builds use source `76f24fe`, with only the baseline kernel replaced
by `8fced24`. Each uses Rust 1.98.0, matching LLVM 22.1.8, explicit native target,
`-C target-cpu=native`, and the existing release options. Separate clean build
directories produced plain and freshly trained PGO builds for each variant.
Default `just pgo-release` passed on both hosts. All four profiles passed
validation before optimization. No builds overlapped timing on the same host.

| Host | Final comparison | Median paired gain | Paired 95% interval |
| --- | --- | ---: | --- |
| Mac M4 Max | Code without PGO | **+64.09%** | [+59.22%, +69.35%] |
| Mac M4 Max | Both builds with fresh PGO | **+34.08%** | [+32.85%, +34.97%] |
| Mac M4 Max | Both builds with fresh PGO, matching target 1 | **+34.24%** | [+33.50%, +37.95%] |
| Mac M4 Max | Direct PGO effect on final code | -0.39% | [-2.45%, +1.84%] |
| Linux Tesla T4 | Code without PGO | **+2.07%** | [+1.59%, +2.44%] |
| Linux Tesla T4 | Both builds with fresh PGO | **+1.41%** | [+0.81%, +2.17%] |
| Linux Tesla T4 | Both builds with fresh PGO, matching target 1 | **+1.98%** | [+1.69%, +2.25%] |
| Linux Tesla T4 | Direct PGO effect on final code | -0.0048% | [-0.0426%, +0.0005%] |

Every code comparison passes both positive-set-median and paired confidence
gates. Each uses two sets of five alternating pairs; the direct PGO control
uses two sets of ten. The primary PGO baseline range is 27.91% of its median on
Mac and 9.24% on T4. Mac desktop activity limits comparisons across separate
run sets. Do not infer a PGO effect by subtracting the plain and PGO code gains.
The direct controls find no repeatable PGO gain or loss. PGO stays mandatory.

All completed samples count 2281701376 hashes after warmup and include normal
result readback. Final NVIDIA PTX loads the target after the Keccak round loop;
the impossible target does not bypass hashing. Matching-target measurements
also pass on both hosts. Each final comparison passes easy minimum-only mining
and difficult maximum-limited fallback checks, with exit codes 0/2 and full ABI
output. Startup and single mining wall-time checks remain separate; no latency
gain is claimed. CREATE2, nonce coverage, output, and batch limits stay intact.

The new CPU oracle checks 225368 GPU scoring results per host, including all
byte values, zero/high-bit patterns, and thresholds 0 through 21. Final release
tests pass on Mac (33 tests) and T4 (32 tests), including existing CREATE2 and
nonce-boundary checks. Formatting, locked check, strict Clippy, cargo-deny, and
`just windows-check` pass. The exact runtime commit's
[Linux CI job](https://github.com/SatoshiAndKin/create2-salt-miner/actions/runs/34479593675/job/102878735028)
passes all 32 PoCL tests, Python checks, and clean default PGO generation and
optimization. The Windows job passes cross-check, then still fails PGO
instrumentation with missing `profiler_builtins` in stock Rust 1.98.0 GNU.
Windows performance remains unmeasured, and its release fix remains incomplete.
The draft PR preserves the required native Windows training/profile/build gate.
