#!/usr/bin/env bash
# Build-only prerequisite for SQLx 0.8.6 sqlite-unbundled/libsqlite3-sys 0.30.1.
# No downloads, system installation, Cargo source edits, CLI or extension artifacts.
# Usage: bash scripts/prepare-sqlite-build.sh --archive FILE --prefix NEW_PATH
#        --target x86_64-unknown-linux-gnu [--cc cc] [--ar ar]
# Source PREFIX/env.sh only after success, with the matching Rust target/features.
# The policy is `xtask sqlite-build` (xtask/src/sqlite_build.rs); the maintained
# `zip` crate reads the authenticated archive. Building xtask needs only its
# own locked crates; the host Cargo target/target-dir overrides do not apply.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
env -u CARGO_BUILD_TARGET -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR \
  cargo build --quiet --locked --manifest-path "$root/xtask/Cargo.toml" --target-dir "$root/xtask/target"
exec "$root/xtask/target/debug/xtask" sqlite-build "$@"
