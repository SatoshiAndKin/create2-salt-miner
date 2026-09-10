# Kernel and dispatch checks on Metal and NVIDIA OpenCL

The initial server comparison found that the retained Mac kernel regressed on
the Tesla T4. See the full baseline and PGO evidence in
[`../ski-lambo-1-2026-09-10`](../ski-lambo-1-2026-09-10).
The user then requested continued kernel work and comparisons on both hosts,
including an audit of settings that should follow the host GPU.

## Device configuration audit

Metal reads `thread_execution_width()` and
`max_total_threads_per_threadgroup()` from the compiled pipeline. It rounds the
chosen group to an execution-width multiple and respects the reported maximum.
It also has a fixed preference of 256 threads per group. OpenCL leaves the local
group size to the driver. The worksize of 71303168 is an explicit number of
hashes per dispatch, not a memory allocation or a Mac model-specific limit.
The solution buffers contain three 32-bit words on Metal and one 64-bit word
on OpenCL; they do not scale with that worksize.

Apple documents the
[pipeline limits and execution width](https://developer.apple.com/documentation/metal/calculating-threadgroup-and-grid-sizes)
as the inputs for valid dispatch sizes. NVIDIA also explains that
[register use and group size affect occupancy](https://developer.download.nvidia.com/compute/DevZone/docs/html/OpenCL/doc/OpenCL_Programming_Guide.pdf).
A reported maximum permits a group size; it does not establish a throughput
gain for this kernel. Runtime measurements determine that result.

## Protocol and controls

Use the existing two-set protocol, with five alternating pairs in each set,
eight warmup batches, 32 timed batches, and 2281701376 completed timed hashes
per sample. Preserve the fixed inputs and worksize. Both set medians must be
positive and the stratified paired bootstrap 95% interval must exclude zero.
Every complete comparison includes separate startup and easy/difficult real
mining checks. Build all variants and run native GPU correctness tests before
starting measurements on each host. Work on the two separate hosts may overlap.

All exploratory builds use Rust 1.98.0, the repository's release profile,
explicit native target, and `-C target-cpu=native`, without PGO. These are code
experiments, not PGO release builds. The final section records fresh PGO
training and direct comparison with the corrected baseline on both hosts.

The device-group candidate changes only Metal's preferred group from
`maximum_threads.min(256)` to the pipeline-reported maximum. It uses the current
retained kernel. The built-in rotation candidate changes only the non-AMD
rotation implementation to `rotate((ulong)x, (ulong)s)`. The compiler-unroll and
unroll-8 candidates each start from the built-in rotation candidate and change
only the round-loop unroll hint. The compiler-unroll candidate removes the
fixed hint so the device compiler can choose. No device-name check or separate
algorithm is introduced.

| Candidate | Host | Reference | Set median changes | Paired median | Paired 95% interval | Result |
| --- | --- | --- | --- | ---: | --- | --- |
| Device-reported maximum group | Mac | Retained kernel and group preference 256 | -0.38%, -0.08% | -0.23% | [-1.23%, +1.06%] | Reject: no measured gain; all 32 native tests passed |
| Built-in rotation, unroll 4 | Mac | Retained kernel | -8.22%, -9.96% | -8.71% | [-10.53%, -7.68%] | Reject as a standalone replacement on Mac |
| Built-in rotation, unroll 4 | Linux | Retained kernel | +15.02%, +14.85% | +14.94% | [+14.45%, +15.60%] | Recovers most of the T4 loss; needs a common replacement |
| Compiler-selected unrolling | Mac | Built-in rotation, unroll 4 | +9.02%, +2.91% | +6.04% | [+1.63%, +16.53%] | Passes the incremental Mac gate |
| Compiler-selected unrolling | Linux | Built-in rotation, unroll 4 | +0.12%, +0.01% | +0.02% | [-0.09%, +0.15%] | No measured Linux effect |
| Unroll 8 | Mac | Built-in rotation, unroll 4 | -19.23%, -27.34% | -24.56% | [-26.86%, -21.09%] | Reject |
| Unroll 8 | Linux | Built-in rotation, unroll 4 | -0.02%, +0.02% | -0.002% | [-0.06%, +0.02%] | No measured Linux effect |
| Union-based rotations | Mac | Built-in rotation, compiler-selected unrolling | +9.98%, +6.00% | +9.22% | [+5.77%, +11.37%] | Passes the Mac gate |
| Union-based rotations | Linux | Built-in rotation, compiler-selected unrolling | -0.61%, +0.02% | +0.01% | [-0.84%, +0.54%] | No repeatable Linux change |
| Packed state | Mac | Built-in rotation, compiler-selected unrolling | -0.14%, +1.18% | +0.52% | [-0.79%, +2.05%] | No repeatable Mac change |
| Packed state | Linux | Built-in rotation, compiler-selected unrolling | +2.88%, +3.07% | +3.00% | [+2.79%, +3.26%] | Passes the Linux gate |
| Word-based zero scoring | Mac | Union rotations and packed state | -0.33%, +0.40% | +0.04% | [-1.66%, +0.81%] | No repeatable Mac change |
| Word-based zero scoring | Linux | Union rotations and packed state | +0.85%, +1.02% | +0.94% | [+0.69%, +1.34%] | Passes the Linux gate |

All shared kernel candidates before word scoring passed 32 native Mac tests
and 31 native Linux tests. Word scoring added one native test per host.
Every comparison completed both sets and its mining checks.
These incremental comparisons do not establish a combined gain against the
corrected baseline. Do not multiply percentages from separate run sets.

The NVIDIA compiler reports 72 registers for the corrected baseline,
built-in rotation, partial-only, and rotation-only kernels, and 75 for the
initial retained kernel. It reports no stack frame or register spills. Each
reports kernel work-group maximum 256, preferred multiple 32, and private
memory size zero. The unroll-hint variants reused a compiler cache entry and
did not repeat register statistics; no separate register count is inferred
from their missing log lines. These resource measurements support further
work on rotation representation and state initialization, but do not alone
explain the full throughput difference.

The union-rotate and packed-state candidates used built-in rotation with
compiler-selected unrolling as a common experimental reference. The union-rotate candidate uses
the existing `nonce_t` union to split and reassemble 32-bit rotation lanes.
The packed-state candidate retries the existing experiment 4 patch, which
initializes the Keccak state with packed words. Each retains the other parts
of the reference unchanged. These retry earlier ideas; the later combined
comparison determines whether to retain them together.

The union/packed combination measured +33.95% against the corrected Mac
baseline, with set medians +37.06%/+33.76% and interval [+33.12%, +39.07%].
Linux measured +0.36%, with mixed set medians +0.39%/-0.11% and interval
[-0.25%, +0.93%]; this did not establish a Linux gain. A direct Mac comparison
against the previous retained code measured +1.29%, with interval
[-0.12%, +1.61%], and did not establish an incremental Mac gain or regression.
This motivated the ninth distinct idea below.

## Ninth idea: score addresses by word

NVIDIA's exported PTX shows 20 separate byte comparisons, predicate-to-integer
conversions, and additions for scoring. The candidate uses five 32-bit words.
Within each byte, adding 0x7f to the low seven bits cannot carry into the next
byte. Combining that result with the original high bit identifies nonzero
bytes. Inverting the high-bit masks and using `popcount` counts exact zero
bytes. This changes scoring only; all Keccak rounds still execute before the
score check. The driver reports 71 registers and no spills for this candidate.

The new GPU reference test checks 10244 address patterns at every threshold
from 0 through 21: **225368 comparisons per host**. It tests every byte value
at every position against dense-zero and no-zero backgrounds, plus adjacent
zero/one and high-bit patterns. The CPU oracle counts individual zero bytes;
it does not reproduce the GPU bit trick. All 33 native Mac tests and 32 native
Linux tests passed, including the existing CREATE2 and nonce-boundary checks.

Exported PTX from the compiler-selected and unroll-8 variants is byte-identical
on this NVIDIA driver. The readable exports in `linux/ptx/` remove only the
trailing NUL terminator. They show a loop on NVIDIA; no claim is made that the
source unroll hint changes code there. The existing fixed hint did affect
measured Metal performance.

The device-group, rotation, unrolling, and packed-state changes are retries or
parameter choices within the original eight ideas. Word scoring is the ninth
distinct idea. Combined comparisons, larger confirmations, benchmark changes,
and PGO work do not count as additional optimization trials.

Commit `76f24fe8763870d9ea5e9bbf84311f307af15ab7` contains the common candidate:
compiler-selected round unrolling, union-based rotations, packed state, and
word scoring. The scalar address, salt, nonce, ABI, and timeout contracts remain
unchanged. It passed formatting, locked check, strict Clippy, all 33 native Mac
tests, cargo-deny, and `just windows-check`. The final PGO comparisons below use
this exact commit. The earlier Windows PGO runtime blocker remains.

`patches/` records each isolated change and `candidate-sources.json` records its
reference and source hashes. The final committed kernel adds line wrapping and
an explanatory comment to the measured word-score candidate.

The helpers in this directory record the exact build and run commands. Raw
measurements and validation logs are split into `mac/` and `linux/`.
`opencl-inspect.rs` uses the existing `ocl` dependency and NVIDIA's
[`-cl-nv-verbose` compiler option](https://registry.khronos.org/OpenCL/extensions/nv/cl_nv_compiler_options.txt)
to report registers and kernel work-group limits. It does not change mining
or benchmark behavior. Unroll hints have the
[documented compiler-hint semantics](https://registry.khronos.org/OpenCL/extensions/nv/cl_nv_pragma_unroll.txt).

## Final code and fresh PGO comparisons

The Mac has an Apple M4 Max with 32 GPU cores and 36 GB of system memory. It
runs macOS 26.2 on AC power with low-power mode disabled. The Linux host is
`ski-lambo-1`, with a Tesla T4, NVIDIA driver 595.71.05, and Ubuntu 24.04.4.
The server's normal workloads and the Mac desktop remained active. No build
or other task-started mining command overlapped timing on the same host.

Both final sources come from `76f24fe`. The baseline replaces only the kernel
with `8fced24:src/kernels/keccak256.cl`. Rust 1.98.0 uses LLVM 22.1.8 and
`-C target-cpu=native`, with explicit native targets, opt-level 3, fat LTO,
one codegen unit, panic abort, and no incremental compilation. Each variant
starts from a separate clean build directory. Both run the unchanged default
`just pgo-release`: real mining with target 1, minimum 30 seconds, and worksize
71303168. Each Mac profile contains 8304 functions with maximum count 3570;
each T4 profile contains 10142 functions with maximum count 24480. Profile
validation and optimization passed for all four profiles.

The fixed factory is `0x0000000000FFe8B47B3e2130213B802212439497`, caller is
`0x0000000000000000000000000000000000000000`, and codehash is
`0x64e604787cbf194841e7b68d7cd28786f6c9a0a3ab9f8b0a0e87cb4387ab0107`.
The unchanged measurement helper uses Python 3.14.7 on Mac and 3.12.3 on Linux.
The main comparison uses target 21. A second PGO comparison uses target 1 to
exercise result handling with matches. These use ten paired samples each,
split into two independent sets. The direct PGO-effect comparison uses twenty
pairs, split into two sets. All samples include completed hashing and normal
result readback. OpenCL's fixed benchmark batches revisit the same nonces;
they still execute the hashes. Production nonce coverage remains unchanged.

| Host | Comparison | Paired median gain | Paired 95% interval | Gate |
| --- | --- | ---: | --- | --- |
| Mac M4 Max | Code, no PGO, target 21 | **+64.09%** | [+59.22%, +69.35%] | Pass |
| Mac M4 Max | Both builds with fresh PGO, target 21 | **+34.08%** | [+32.85%, +34.97%] | Pass |
| Mac M4 Max | Both builds with fresh PGO, target 1 | **+34.24%** | [+33.50%, +37.95%] | Pass |
| Mac M4 Max | PGO effect on final code | -0.39% | [-2.45%, +1.84%] | No repeatable effect |
| Linux Tesla T4 | Code, no PGO, target 21 | **+2.07%** | [+1.59%, +2.44%] | Pass |
| Linux Tesla T4 | Both builds with fresh PGO, target 21 | **+1.41%** | [+0.81%, +2.17%] | Pass |
| Linux Tesla T4 | Both builds with fresh PGO, target 1 | **+1.98%** | [+1.69%, +2.25%] | Pass |
| Linux Tesla T4 | PGO effect on final code | -0.0048% | [-0.0426%, +0.0005%] | No repeatable effect |

Both sets have positive medians for each passing comparison. The confidence
intervals use 20000 paired bootstrap samples, stratified by set, with seed
71303168. The primary PGO rate medians are 490.44/657.41 million hashes/s on
Mac and 663.70/673.15 million hashes/s on T4, for baseline/candidate. Gains
come from paired ratios, not ratios of these separate medians. Baseline ranges
are 27.91% and 9.24% of their medians respectively. This variation limits
comparisons across separate run sets, especially on the Mac. Do not subtract
the PGO and plain code gains to infer a PGO effect. The direct control finds
no repeatable PGO throughput gain or loss. PGO remains mandatory.

All final comparisons passed real target-1 minimum-only mining and target-21
maximum-limited fallback checks, with exits 0 and 2 and full 96-byte ABI output.
The primary PGO comparison's easy mining wall times are 2.89/2.02 seconds on
Mac and 2.95/2.91 seconds on T4. Difficult mining takes 3.12/3.91 seconds on Mac
and 3.85/3.81 seconds on T4. These single checks establish behavior; they do
not establish a latency gain. Limits apply at batch boundaries and exclude
setup time. Startup estimates remain separate; no startup gain is claimed.

The final NVIDIA PTX in `final/linux/final-kernel.ptx` loads the target after
the 23-round loop and completes the partial final round before scoring. It
uses five `popc.b32` instructions. The impossible target therefore does not
bypass hashing in this measured program. The driver reports 71 registers,
no stack frame, and no spills. Kernel, binary, profile, source-archive,
lockfile, and measurement-script hashes are in each host's `artifacts.json`.
Each of the eight measured binaries contains the expected kernel bytes.

Nine distinct ideas were tested across the complete run. Four remain: a round
loop with compiler-selected unrolling, union-based paired rotations, packed
state initialization, and word-based scoring. Five remain rejected: invariant
Metal bindings, packed dispatch arguments, threadgroup tuning, reduced chi
temporary storage, and the solution atomic guard. Repeated measurements,
parameter sweeps, PGO work, and combined comparisons do not add to that count.

## Final validation and release limit

All 33 native Mac tests and 32 native T4 tests passed for both final reference
and candidate source variants in release mode. These include 28 CPU tests,
CREATE2/nonce-boundary checks, mining limits, and the new scoring oracle.
Formatting, locked check, strict Clippy, cargo-deny, and Windows GNU cross-check
passed. Cargo-deny retains existing duplicate/yanked-dependency warnings.

The [Linux CI job for the exact runtime commit](https://github.com/SatoshiAndKin/create2-salt-miner/actions/runs/34479593675/job/102878735028)
passed all 32 PoCL tests, required Rust/Python checks, and clean default PGO
generation and optimization. Its profile contains 10245 functions with maximum
count 1020. CI uses `-C target-cpu=x86-64`; native measurements use the explicit
native flags above. The
[Windows job](https://github.com/SatoshiAndKin/create2-salt-miner/actions/runs/34479593675/job/102878734667)
passed cross-check, then failed instrumentation with E0463 because stock Rust
1.98.0 GNU lacks `profiler_builtins`. The workflow is therefore not fully green.

Windows performance remains unmeasured. The Windows release fix remains
incomplete until a supported target/compiler decision permits instrumentation,
native training, a committed fresh profile, and a successful PGO release build.
The PR remains a draft. No production service was replaced or deployed.

`final/summary.json` records all eight final results, set medians, baseline
variation, startup estimates, and mining wall times. `final/mac/` and
`final/linux/` contain the raw pairs, exact commands, environment, build/profile
logs, native tests, and artifact fingerprints. `validation/ci.json` records
the completed CI outcomes; its companion excerpt preserves the test/profile
counts and Windows error.
