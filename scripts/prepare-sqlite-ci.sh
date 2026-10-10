#!/usr/bin/env bash
# Build-only entry: bash scripts/prepare-sqlite-ci.sh [--parent OWNED_DIR] -- cargo build --locked
# Requires native GCC/ar, cargo/rustc, curl and LIBCLANG_PATH (Ubuntu 26.04: llvm-18).
# Never installs host packages or changes Cargo.
# The reviewed helper (`xtask sqlite-build`) owns all source hashes, C flags and
# SQLite export policy; this entry is `xtask sqlite-ci` (xtask/src/sqlite_ci.rs).
# Building xtask needs only its own locked crates; the host Cargo
# target, target-dir and rustflags overrides apply to the consumer command,
# not to xtask.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
env -u CARGO_BUILD_TARGET -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR \
  -u RUSTFLAGS -u CARGO_ENCODED_RUSTFLAGS -u CARGO_BUILD_RUSTFLAGS \
  cargo build --quiet --locked --manifest-path "$root/xtask/Cargo.toml" --target-dir "$root/xtask/target"
exec "$root/xtask/target/debug/xtask" sqlite-ci "$@"
