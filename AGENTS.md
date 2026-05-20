# AGENTS

## Setup
- Use stable Rust (`rust-toolchain.toml`) and install dependencies with `cargo`.
- Install an OpenCL runtime/SDK and ensure it is available in `PATH`.
- Configure mining inputs via `salty.toml` or CLI flags (`mine` command); CLI args override config.
- Validate OpenCL device visibility with: `cargo run --release -- list`.

## Validation
- `cargo fmt --all -- --check`
- `cargo check --locked`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo nextest run --locked`
- `cargo deny check`
- Windows cross-check: `just windows-check`

## Cross-compilation and release notes
- Windows target is `x86_64-pc-windows-gnu` via `cross` and `Dockerfile.cross` (`Cross.toml` image override).
- Build Windows artifact with: `just windows`.
- Release automation lives in `.github/workflows/release.yml`.

## Repo conventions
- This is a Rust binary crate; use `eyre` for error handling.
- Keep mining-path changes performance-aware (avoid unnecessary host/kernel synchronization).
- Keep configuration behavior consistent between CLI and `salty.toml`.
