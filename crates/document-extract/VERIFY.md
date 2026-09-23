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

Inbox `msg_c2ccab3538dc` / `delivery_e650d7360f76` addressed: running separator count, Partial for depth/shape/out-of-range cells, stdout bound `500_000*6+4096` with Output on overflow.
Heavy DB gate not run.
