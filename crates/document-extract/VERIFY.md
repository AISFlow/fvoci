# document-extract follow-up verification

Worktree-local, cwd `crates/document-extract`, `CARGO_TARGET_DIR=$PWD/target`.
Rust 1.98.1. Pin `edwardkim/rhwp` `e8800c8def63449808a4092798442652ed460552`.
Native extraction is **not** product-accepted.

```sh
export CARGO_HOME=/home/kinesis/orca/toolchains/fvoci-rust/cargo
export RUSTUP_HOME=/home/kinesis/orca/toolchains/fvoci-rust/rustup
export PATH="$CARGO_HOME/bin:$PATH"
cd crates/document-extract
export CARGO_TARGET_DIR="$PWD/target"
```

| Command | Elapsed | Exit | Notes |
| --- | --- | --- | --- |
| `rustfmt --edition 2021 src/*.rs src/bin/*.rs tests/*.rs` | <1s | 0 | owned sources only |
| `cargo test --locked --offline -- --test-threads=1` | 2.34s | 0 | 32 passed; production bin rejects `--test-hang-ms` / `--dump-rlimits` |
| `cargo test --locked --offline --features test-hang -- --test-threads=1` | 2.85s | 0 | 32 passed; `extract_killable` timeout reaps product helper (zombie fails); RLIMIT_AS dump |
| `cargo clippy --locked --offline --all-targets --features test-hang -- -D warnings` | 0.32s | 0 | after WalkState refactor |
| `cargo build --locked --offline` | 0.16s | 0 | production profile, no `test-hang` |
| `$CARGO_TARGET_DIR/debug/document-extract --name 안녕.hwp < fixtures/user-hancom-12.30-안녕.hwp` | <0.01s | 0 | `ok` text `안녕`, `used_preview_stream=false` |
| `$CARGO_TARGET_DIR/debug/document-extract --name 안녕.hwpx < fixtures/user-hancom-12.30-안녕.hwpx` | <0.01s | 0 | `ok` text `안녕`, `used_preview_stream=false` |
| production bin `--test-hang-ms` / `--dump-rlimits` | — | 2 | `unknown arg` |

Inbox addressed:
- `msg_c2ccab3538dc` / `delivery_e650d7360f76`: running separator count, Partial for depth/shape/out-of-range cells, stdout bound `500_000*6+4096` with Output on overflow.
- `msg_70f6f5c8ed3f` / `delivery_502a0eac65b5`: warm scoped tests already run; first-party CLI HWP+HWPX independently confirmed `안녕`.
- `msg_64711681315e` (same delivery): stayed on this branch; no root/CI edits; incremental commit only under `crates/document-extract/**`.

Heavy DB gate not run. Native extraction is not product-accepted.
