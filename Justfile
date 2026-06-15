set dotenv-load

windows-target := "x86_64-pc-windows-gnu"
portable-rustflags := "-C target-cpu=x86-64"

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
