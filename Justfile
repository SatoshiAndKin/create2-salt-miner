set dotenv-load

windows-target := "x86_64-pc-windows-gnu"
portable-rustflags := "-C target-cpu=x86-64"
train_factory := env_var_or_default("TRAIN_FACTORY", "0x0000000000FFe8B47B3e2130213B802212439497")
train_caller := env_var_or_default("TRAIN_CALLER", "0x0000000000000000000000000000000000000000")
train_codehash := env_var_or_default("TRAIN_CODEHASH", "0x64e604787cbf194841e7b68d7cd28786f6c9a0a3ab9f8b0a0e87cb4387ab0107")
train_worksize := env_var_or_default("TRAIN_WORKSIZE", "0x4400000")
train_zeros := env_var_or_default("TRAIN_ZEROS", "1")
train_min_runtime_secs := env_var_or_default("TRAIN_MIN_RUNTIME_SECS", "30")
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

windows:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cross >/dev/null 2>&1; then
      cargo install cross --locked
    fi
    docker buildx build \
      --platform linux/amd64 \
      --tag cross-custom-create-salt-miner:x86_64-pc-windows-gnu \
      --load \
      -f Dockerfile.cross .
    CROSS_CONTAINER_OPTS="--platform linux/amd64" RUSTFLAGS="{{portable-rustflags}}" cross build --release --target {{windows-target}}

windows-check:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cross >/dev/null 2>&1; then
      cargo install cross --locked
    fi
    docker buildx build \
      --platform linux/amd64 \
      --tag cross-custom-create-salt-miner:x86_64-pc-windows-gnu \
      --load \
      -f Dockerfile.cross .
    CROSS_CONTAINER_OPTS="--platform linux/amd64" RUSTFLAGS="{{portable-rustflags}}" cross check --target {{windows-target}}

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
    cargo pgo build -- --bin salty

pgo-train:
    #!/usr/bin/env bash
    set -euo pipefail
    host_triple="$(rustc -vV | sed -n 's/^host: //p')"
    instrumented_bin="target/${host_triple}/release/salty"
    if [[ ! -x "${instrumented_bin}" ]]; then
      echo "missing instrumented binary at ${instrumented_bin}; run 'just pgo-instrument' first" >&2
      exit 1
    fi
    "${instrumented_bin}" mine \
      --factory "{{train_factory}}" \
      --caller "{{train_caller}}" \
      --codehash "{{train_codehash}}" \
      --worksize "{{train_worksize}}" \
      --zeros "{{train_zeros}}" \
      --min-runtime-secs "{{train_min_runtime_secs}}" \
      --abi

pgo-optimize:
    #!/usr/bin/env bash
    set -euo pipefail
    if ! command -v cargo-pgo >/dev/null 2>&1; then
      cargo install cargo-pgo --locked
    fi
    cargo pgo optimize -- --bin salty

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
    cargo pgo bolt build --with-pgo -- --bin salty

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
    bolt_instrumented_bin="target/${host_triple}/release/salty-bolt-instrumented"
    if [[ ! -x "${bolt_instrumented_bin}" ]]; then
      echo "missing BOLT-instrumented binary at ${bolt_instrumented_bin}; run 'just bolt-instrument' first" >&2
      exit 1
    fi
    "${bolt_instrumented_bin}" mine \
      --factory "{{train_factory}}" \
      --caller "{{train_caller}}" \
      --codehash "{{train_codehash}}" \
      --worksize "{{train_worksize}}" \
      --zeros "{{train_zeros}}" \
      --min-runtime-secs "{{train_min_runtime_secs}}" \
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
    cargo pgo bolt optimize --with-pgo -- --bin salty
