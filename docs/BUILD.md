# MAI Build Guide

## Prerequisites

- Rust 1.78+ (stable toolchain)
- Python 3.11+ with `pip >= 26.1` (run `python -m pip install --upgrade pip`
  on a fresh checkout — pip 26.0.1 carries CVE-2026-3219 and
  CVE-2026-6357, fixed in 26.1)
- protoc (Protocol Buffers compiler, for gRPC)
- pkg-config + OpenSSL dev headers (Linux: `libssl-dev`)

## Quick Start

```bash
cd mai/

# Type check (fast, no artifacts)
cargo check --workspace

# Full build (debug)
cargo build --workspace

# Lint
cargo clippy --workspace -- -D warnings

# Format check (CI gate)
cargo fmt --all -- --check

# Run tests
cargo test --workspace
```

## Cargo.lock

`Cargo.lock` is committed to the repository. This ensures reproducible
builds across all environments. Do not add it to `.gitignore`.

After adding or updating dependencies, commit the updated lock file:

```bash
cargo update  # or cargo add <crate>
git add Cargo.lock
git commit -m "chore: update Cargo.lock"
```

## Tonic / gRPC Dependencies

The gRPC stack uses tonic 0.12.x with prost for code generation. Known
dependency resolution notes:

- `tonic` and `tonic-build` versions must match (both 0.12.x)
- `prost` and `prost-build` must match their tonic-compatible versions
- If `cargo check` shows tonic version conflicts, run:

```bash
cargo update -p tonic
cargo update -p prost
```

## Python SDK

```bash
cd mai/mai-sdk-python/

# Install in development mode
pip install -e ".[dev]"

# Lint
ruff check .

# Type check
mypy src/

# Run tests
pytest tests/
```

## Configuration Files

- `config/adapters.toml` -- Adapter backend configuration
- `config/auth_keys.toml` -- API key authentication (Session 14c)
- `config/server.toml` -- Server settings (ports, tier, air-gap)

## Ports

- REST API: 8420 (default)
- gRPC API: 8421 (default)

## Formatting

Run `cargo fmt --all` before committing Rust code. The CI pipeline
checks formatting and will reject PRs with drift.

Known issue: `cargo fmt` may reformat generated protobuf code in
`mai-api/src/grpc/`. If this happens, add `#[rustfmt::skip]` to the
generated module declaration or exclude the file in `rustfmt.toml`.
