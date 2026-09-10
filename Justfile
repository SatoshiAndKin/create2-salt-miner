set dotenv-load

target_directory := env_var_or_default("CARGO_TARGET_DIR", "target")

windows-target := "x86_64-pc-windows-gnu"
export WINDOWS_RUSTFLAGS := "-C target-cpu=x86-64"
export TRAIN_FACTORY := env_var_or_default("TRAIN_FACTORY", "0x0000000000FFe8B47B3e2130213B802212439497")
export TRAIN_CALLER := env_var_or_default("TRAIN_CALLER", "0x0000000000000000000000000000000000000000")
export TRAIN_CODEHASH := env_var_or_default("TRAIN_CODEHASH", "0x64e604787cbf194841e7b68d7cd28786f6c9a0a3ab9f8b0a0e87cb4387ab0107")
export TRAIN_WORKSIZE := env_var_or_default("TRAIN_WORKSIZE", "71303168")
export TRAIN_ZEROS := env_var_or_default("TRAIN_ZEROS", "1")
export TRAIN_MIN_RUNTIME_SECS := env_var_or_default("TRAIN_MIN_RUNTIME_SECS", "30")
enable_bolt := env_var_or_default("ENABLE_BOLT", "0")

fmt:
    cargo fmt --all -- --check

clippy:
    cargo clippy --all-targets --all-features -- -D warnings

test:
    cargo test --all-features

eta:
    cargo test eta

validate: fmt clippy test eta

windows: _windows-setup
    #!/usr/bin/env bash
    set -euo pipefail
    python3 scripts/pgo.py verify
    CROSS_CONTAINER_OPTS="--platform linux/amd64 -v ${PWD}/.pgo:/salty-pgo:ro" RUSTFLAGS="{{WINDOWS_RUSTFLAGS}} -Cprofile-use=/salty-pgo/salty-windows-x86_64.profdata -Cllvm-args=-pgo-warn-missing-function" cross build --release --locked --target {{windows-target}}

windows-check: _windows-setup
    #!/usr/bin/env bash
    set -euo pipefail
    CROSS_CONTAINER_OPTS="--platform linux/amd64" RUSTFLAGS="{{WINDOWS_RUSTFLAGS}}" cross check --locked --target {{windows-target}}

pgo-info:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-pgo >/dev/null 2>&1; then
      cargo install cargo-pgo --locked
    fi
    if ! cargo pgo info; then
      echo "cargo-pgo reported missing optional tooling; continuing with baseline PGO path"
    fi

pgo-instrument:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-pgo >/dev/null 2>&1; then
      cargo install cargo-pgo --locked
    fi
    RUSTFLAGS="${RUSTFLAGS:--C target-cpu=native}" cargo pgo build -- --locked --bin salty

pgo-train:
    #!/usr/bin/env bash
    set -euo pipefail
    host_triple="$(rustc -vV | sed -n 's/^host: //p')"
    instrumented_bin="{{target_directory}}/${host_triple}/release/salty"
    if [[ ! -x "${instrumented_bin}" ]]; then
      echo "missing instrumented binary at ${instrumented_bin}; run 'just pgo-instrument' first" >&2
      exit 1
    fi
    profiles_dir="$(cd "{{target_directory}}/pgo-profiles" && pwd)"
    export LLVM_PROFILE_FILE="${profiles_dir}/salty-%m-%p.profraw"
    "${instrumented_bin}" mine \
      --factory "{{TRAIN_FACTORY}}" \
      --caller "{{TRAIN_CALLER}}" \
      --codehash "{{TRAIN_CODEHASH}}" \
      --worksize "{{TRAIN_WORKSIZE}}" \
      --zeros "{{TRAIN_ZEROS}}" \
      --min-runtime-secs "{{TRAIN_MIN_RUNTIME_SECS}}" \
      --abi

pgo-optimize:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-pgo >/dev/null 2>&1; then
      cargo install cargo-pgo --locked
    fi
    RUSTFLAGS="${RUSTFLAGS:--C target-cpu=native}" cargo pgo optimize -- --locked --bin salty

bolt-preflight:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "{{enable_bolt}}" != "1" ]]; then
      echo "SKIP: BOLT disabled (set ENABLE_BOLT=1 to enable)"
      exit 0
    fi
    missing=()
    command -v llvm-bolt >/dev/null 2>&1 || missing+=("llvm-bolt")
    command -v merge-fdata >/dev/null 2>&1 || missing+=("merge-fdata")
    if (( ${#missing[@]} > 0 )); then
      echo "SKIP: missing BOLT deps: ${missing[*]}"
      exit 0
    fi
    echo "BOLT preflight OK"

bolt-instrument:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "{{enable_bolt}}" != "1" ]]; then
      echo "SKIP: BOLT disabled (set ENABLE_BOLT=1 to enable)"
      exit 0
    fi
    if ! command -v llvm-bolt >/dev/null 2>&1 || ! command -v merge-fdata >/dev/null 2>&1; then
      echo "SKIP: missing BOLT deps (need llvm-bolt and merge-fdata)"
      exit 0
    fi
    if ! command -v cargo-pgo >/dev/null 2>&1; then
      cargo install cargo-pgo --locked
    fi
    RUSTFLAGS="${RUSTFLAGS:--C target-cpu=native}" cargo pgo bolt build --with-pgo -- --locked --bin salty

bolt-train:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "{{enable_bolt}}" != "1" ]]; then
      echo "SKIP: BOLT disabled (set ENABLE_BOLT=1 to enable)"
      exit 0
    fi
    if ! command -v llvm-bolt >/dev/null 2>&1 || ! command -v merge-fdata >/dev/null 2>&1; then
      echo "SKIP: missing BOLT deps (need llvm-bolt and merge-fdata)"
      exit 0
    fi
    host_triple="$(rustc -vV | sed -n 's/^host: //p')"
    bolt_instrumented_bin="{{target_directory}}/${host_triple}/release/salty-bolt-instrumented"
    if [[ ! -x "${bolt_instrumented_bin}" ]]; then
      echo "missing BOLT-instrumented binary at ${bolt_instrumented_bin}; run 'just bolt-instrument' first" >&2
      exit 1
    fi
    "${bolt_instrumented_bin}" mine \
      --factory "{{TRAIN_FACTORY}}" \
      --caller "{{TRAIN_CALLER}}" \
      --codehash "{{TRAIN_CODEHASH}}" \
      --worksize "{{TRAIN_WORKSIZE}}" \
      --zeros "{{TRAIN_ZEROS}}" \
      --min-runtime-secs "{{TRAIN_MIN_RUNTIME_SECS}}" \
      --abi

bolt-optimize:
    #!/usr/bin/env bash
    set -euo pipefail
    if [[ "{{enable_bolt}}" != "1" ]]; then
      echo "SKIP: BOLT disabled (set ENABLE_BOLT=1 to enable)"
      exit 0
    fi
    if ! command -v llvm-bolt >/dev/null 2>&1 || ! command -v merge-fdata >/dev/null 2>&1; then
      echo "SKIP: missing BOLT deps (need llvm-bolt and merge-fdata)"
      exit 0
    fi
    if ! command -v cargo-pgo >/dev/null 2>&1; then
      cargo install cargo-pgo --locked
    fi
    RUSTFLAGS="${RUSTFLAGS:--C target-cpu=native}" cargo pgo bolt optimize --with-pgo -- --locked --bin salty

# Fresh profiles, measured training, profile validation, then optimization.
pgo-release:
    just pgo-instrument
    just pgo-train
    just pgo-merge
    just pgo-optimize

pgo-merge:
    #!/usr/bin/env bash
    set -euo pipefail
    llvm_profdata="$(rustc --print target-libdir)/../bin/llvm-profdata"
    profiles_dir="{{target_directory}}/pgo-profiles"
    shopt -s nullglob
    raw_profiles=("${profiles_dir}"/*.profraw)
    (( ${#raw_profiles[@]} > 0 )) || { echo "No fresh PGO profiles found" >&2; exit 1; }
    "${llvm_profdata}" merge -o "${profiles_dir}/checked.profdata" "${raw_profiles[@]}"
    python3 scripts/pgo.py check "${profiles_dir}/checked.profdata"

windows-pgo-instrument: _windows-setup
    #!/usr/bin/env bash
    set -euo pipefail
    CROSS_CONTAINER_OPTS="--platform linux/amd64" RUSTFLAGS="{{WINDOWS_RUSTFLAGS}} -Cprofile-generate" cross build --release --locked --target {{windows-target}}
    python3 scripts/pgo.py bundle target/windows-pgo-bundle.zip

windows-pgo-train bundle:
    powershell.exe -NoProfile -File scripts/windows-pgo-train.ps1 {{quote(bundle)}}

windows-pgo-import results:
    python3 scripts/pgo.py import {{quote(results)}}

_windows-setup:
    #!/usr/bin/env bash
    set -euo pipefail
    rustup toolchain install 1.98.0-x86_64-unknown-linux-gnu --force-non-host --profile minimal
    if ! command -v cross >/dev/null 2>&1; then
      cargo install cross --locked
    fi
    docker buildx build \
      --platform linux/amd64 \
      --tag cross-custom-create-salt-miner:x86_64-pc-windows-gnu \
      --load \
      -f Dockerfile.cross .
