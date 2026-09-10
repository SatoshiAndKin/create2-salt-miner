# ⛏️ Salty – CREATE2 Salt Miner

Nickname: `salty`

An _extremely_ fast miner for finding salts that create gas-efficient and vanity Ethereum addresses via `CREATE2`.

Salty only searches for results better than what is already found. For example, if a salt is found that results in an address with 3 zero bytes anywhere in the address, the next salt will only be displayed if it results in an address with at least 4 zero bytes anywhere. This improves performance by reducing the number of times the kernel needs to communicate with the host.

Salty can run for a really long time and will keep finding better salts. It is recommended to leave it running for a few hours if you're looking to find a salt that results in an efficient address.

Salty is written in [Rust](https://www.rust-lang.org/) and uses [Alloy](https://github.com/alloy-rs/core) for Ethereum primitives. It uses Metal on macOS and [OpenCL](https://www.khronos.org/opencl/) on Linux and Windows.

On macOS, Salty uses the native Metal compute API and selects the system GPU. Apple deprecated OpenCL in macOS 10.14, and current macOS releases can expose no usable OpenCL device. Metal keeps GPU mining available on Apple silicon.

On Linux and Windows, OpenCL lets Salty use CPUs, GPUs, and other supported accelerators. A GPU is usually much faster than a CPU. CPU mining requires an OpenCL driver for the processor.

## Usage

Salty is currently tested on Linux, macOS, and Windows. It works with Metal GPUs on macOS and OpenCL devices on Linux and Windows.

You need Rust. Linux and Windows builds also need an OpenCL SDK. Start by cloning the repository.

```bash
git clone git@github.com:akshatmittal/create2-salt-miner.git
```

You can then run it with `cargo` by providing each option as an argument.

```bash
cargo run --release -- mine --factory 0x0000000000FFe8B47B3e2130213B802212439497                            \
                            --caller 0xd8dA6BF26964aF9D7eEd9e03E53415D37aA96045                             \
                            --codehash 0x64e604787cbf194841e7b68d7cd28786f6c9a0a3ab9f8b0a0e87cb4387ab0107
```

Alternatively, you can create a file named `salty.toml` in root with the same parameter names and simply run the miner. A sample file is included in this repo. CLI arguments take precedence so any arguments you provide via the CLI will override the arguments provided via the config file.

```bash
cargo run --release -- mine
```

To run the miner against a remote Salty HTTP server, start the server on the
remote machine and set `remote_server` in `salty.toml` or pass it on the CLI.

```toml
remote_server = "http://127.0.0.1:3000"
```

```bash
cargo run --release -- serve --host 0.0.0.0 --port 3000
cargo run --release -- mine --remote-server http://127.0.0.1:3000
```

The `list` command displays the available accelerator devices and the selected device.

```bash
cargo run --release -- list
```

## Features

- [x] Multiple Config Sources (CLI, Config File)
- [x] Metal Backend (macOS GPU)
- [x] OpenCL Backend (CPU, GPU, Accelerators)
- [x] Ranking Mode (Zero Bytes)
- [ ] Ranking Mode (Any Bytes)
- [ ] Pattern Matching Mode
- [x] CREATE2 Support
- [ ] Hardhat Plugin
- [ ] Foundry Plugin
- [ ] CREATE3 Support
- [ ] WASM Build (is that even possible?)

## Parameters

The following parameters are available when using the `mine` command.

| Option     | Description                                                          | Default                                      |
| ---------- | -------------------------------------------------------------------- | -------------------------------------------- |
| `factory`  | Factory address that will be used to deploy the contract via CREATE2 | `0x0000000000FFe8B47B3e2130213B802212439497` |
| `caller`   | Caller for the deployment                                            | (required parameter)                         |
| `codehash` | Keccak-256 hash of the contract initialization code                  | (required parameter)                         |
| `worksize` | Work size per batch                                                  | `0x4400000`                                  |
| `zeros`    | Minimum zero bytes to look for in the created contract (no stop)     | `1`                                          |
| `remote_server` | Remote Salty HTTP server base URL used by `mine` instead of local OpenCL mining | unset                              |
| `min_runtime_secs` | Mine for at least this many seconds, then return the best qualifying result found | (disabled) |
| `max_runtime_secs` | Return the best candidate at the maximum, including a below-target candidate | (disabled) |

Both runtime limits work with local and remote mining. Without a minimum, a
qualifying result can return immediately. A maximum takes precedence over a
longer minimum. The miner checks limits at batch boundaries; setup, queue, and
network time do not count. A zero minimum is invalid; a zero maximum stops at
the first batch boundary.

At the maximum, the CLI prints the best candidate and exits with code `2` if its
score is below the target. If no candidate exists, it also exits with code `2`.
ABI output keeps the `abi.encode(bytes32,address,uint256)` format. The HTTP
response keeps `found`, `salt`, `address`, `score`, `runtime_ms`, and `cache_hit`.
`found` means a candidate exists; compare `score` with `zeros` to check the target.
`runtime_ms` reports measured mining time, also when no candidate exists. Cache
keys include both limits. See `/api-docs/openapi.json` for the API schema.

## Performance Benchmarks

Run `salty bench` for warmed, completed hashes per second. The default
`--zeros 21` hashes without matches. Use `--zeros 1` to include frequent
solution writes, or `--zeros 0` to exercise the initial fallback workload.
Compare the same inputs, worksize, warmup count, batch count, and target.
The benchmark includes solution-buffer readback. Measure startup separately.

| Platform          | Platform Type  | Speed |
| ----------------- | -------------- | ----- |
| Nvidia RTX 3070   | GPU (CUDA)     | 1,250 |
| Apple M1 Pro      | Hybrid (Metal) | 40    |
| AMD Ryzen 5 3600  | CPU (PoCL)     | TODO  |
| AMD Ryzen 9 5900X | CPU (PoCL)     | TODO  |

Speed is measured in million attempts per second.

## Acknowledgements

This project is heavily inspired by 0age's `create2crunch`. The code for the OpenCL Kernel is taken from there and modified to work in this context.

- [0age](https://github.com/0age)
- [Khronos OpenCL SDK](https://github.com/KhronosGroup/OpenCL-SDK)

## Profile-guided releases

Use Rust `1.98.0` and its `llvm-tools-preview` component. PGO is required for
Linux and Windows release artifacts. Python 3.9+, `just`, and `cargo-pgo` are
build tools; native Windows training needs PowerShell, `just`, and an installed
OpenCL driver.

`just pgo-release` runs instrumentation, training, profile merging and validation,
and optimization in order. It removes old raw profiles before training. Linux
release CI runs this helper with PoCL. Optional BOLT uses those same Linux raw
profiles. `TRAIN_WORKSIZE` defaults to `71303168` (TOML `0x4400000`) for PGO and
BOLT. Other training inputs use the `TRAIN_*` variables in `Justfile`. Keep
`RUSTFLAGS` the same for every step; Linux release CI uses `-C target-cpu=x86-64`.

Windows PGO is currently blocked. Stock Rust `1.98.0` does not include
`profiler_builtins` for `x86_64-pc-windows-gnu`. `just windows-check` passes, but
`just windows-pgo-instrument` fails with compiler error E0463. Rust enables the
profiling runtime for its MSVC and GNU LLVM distributions; keeping the current
GNU target requires a custom Rust build with profiling enabled. See the
[Rust 1.98.0 build configuration](https://github.com/rust-lang/rust/blob/1.98.0/src/ci/github-actions/jobs.yml#L659-L738).
The Windows target decision, native training, and a successful PGO release build
remain open. No valid Windows profile or training bundle has been produced.

After resolving that compiler requirement, use this training sequence after the
final source and lockfile changes:

1. Run `just windows-pgo-instrument` on the cross-build host. It writes
   `target/windows-pgo-bundle.zip` with the instrumented GNU executable, compiler
   identity, source and lockfile hashes, compiler flags, and training inputs.
2. Copy the bundle to native Windows. From this checkout, run
   `just windows-pgo-train <bundle.zip>`. You can also extract the bundle to an
   empty directory and use its included `Justfile` with the original zip path.
   The helper checks the executable, uses an empty working directory, lists
   OpenCL devices, and mines with the recorded inputs. It writes
   `<bundle.zip>.results.zip` with fresh raw profiles and training metadata.
3. Copy the results back. Run `just windows-pgo-import <results.zip>`. The helper
   rejects changed sources, lockfiles, compilers, flags, or training inputs. It
   merges with Rust's matching LLVM tool and rejects empty or invalid profiles.
4. Run `just windows`. It checks metadata and the profile checksum, then mounts
   `.pgo` at `/salty-pgo` inside the cross container for the optimized build.
5. Commit `.pgo/salty-windows-x86_64.profdata` and its `.json` metadata together.
   Retrain after source, lockfile, or build-input changes, including release
   version changes. A missing or stale profile blocks the Windows release.

A cross-build does not validate an OpenCL driver or Windows performance. Run the
bundle on the Windows machine that will mine.
