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
| `cargo test --locked --offline` | 2.72s | 0 | 50 passed (7 lib + 35 extract + 8 boundary); no `--test-threads=1`; production bin rejects `--test-hang-ms` / `--dump-rlimits` |
| `cargo test --locked --offline --features test-hang` | 3.48s | 0 | 50 passed; `extract_killable` timeout reaps product helper; RLIMIT_AS dump; no `--test-threads=1` |
| `cargo clippy --locked --offline --all-targets --features test-hang -- -D warnings` | 1.69s | 0 | |

Those numbers are the original follow-up record at that worker SHA. After PR8
(`125ef25`) the 3 child-IO lib tests live in `document-extract-client`; parser
lib tests are those 3 fewer. Native extraction is **not** product-accepted.

Cancellation/parent-death follow-up (this worktree, SHA after commits). Worktree-local
`CARGO_TARGET_DIR`. Client target is `crates/document-extract-client/target`.
Native target is `crates/document-extract/target`. Attachment HTTP was not
exercised.

| Command | Elapsed | Exit | Notes |
| --- | --- | --- | --- |
| client `cargo fmt --check` | <1s | 0 | owned crate |
| client `cargo clippy --locked --offline --all-targets -- -D warnings` | 3.27s first / 0.29s later | 0 | default clippy requested by PR8 review |
| client `cargo clippy --locked --offline --all-targets --features test-hang -- -D warnings` | 0.36s / 0.31s | 0 | |
| client `cargo test --locked --offline --all-targets` | 2.44s first / 0.36s later | 0 | 8 passed |
| client `cargo test --locked --offline --all-targets --features test-hang` | 0.70s / 0.49s | 0 | 8 passed |
| `sh fetch-rhwp.sh` | 29.89s | 0 | pin `e8800c8`, worktree vendor only |
| native `cargo fetch --locked` | 0.14s | 0 | |
| native `cargo clippy --locked --offline --all-targets -- -D warnings` | 34.11s | 0 | first rhwp compile in this target |
| native `cargo clippy --locked --offline --all-targets --features test-hang -- -D warnings` | 0.78s | 0 | |
| native `cargo test --locked --offline --all-targets` | 62.72s | 0 | 5 lib + 37 extract + 9 boundary = 51; production helper rejects `--test-hang-ms` / `--dump-rlimits`; parent-death driver not built |
| native `cargo test --locked --offline --all-targets --features test-hang` | 4.86s | 0 | 5 lib + 37 extract + 12 boundary = 54; cancel before spawn / while slot wait / running hang; parent SIGKILL reaps helper; next extract works |

Client 8 = previous 6 + cancel-before-admission + unset-cancel still MissingExecutable.
Native default 51 = post-PR8 49 + metadata pin + cancel-before-spawn.
Native test-hang 54 = post-PR8 49 + metadata pin + cancel-before-spawn + slot-wait cancel + running-hang cancel + parent-SIGKILL.
Child-IO tests remain in the client crate. Product HTTP integration is a different task.

B1: HWP5 `raw_stream.is_none()` and HWPX empty paragraph list map parser `Section::default()` drops to `Partial` (recovered body) or `Corrupt` (none). Genuine empty fixtures still `Empty`. At this historical worker SHA, zero-paragraph HWPX was classified conservatively. The subsequent coordinator fix rechecks ambiguous sections with public rhwp APIs, distinguishing genuine empty from parse failure; see docs/rewrite.md for final integration evidence.

B2: table/picture captions walked; Top/Left before row-major cells, Bottom/Right after.

M2: warnings collapsed per kind (×N), cap 32; stdout `pipe exceeded bound` classified as `resource_limit/output` before ChildCrash.

M3: SIGABRT → Memory only with `memory allocation of` + `failed` on stderr.

M4: equation script, form caption/text, picture caption extracted; HiddenComment not walked.

L1: helper `stdin().take(max_input+1)`.

Inbox `msg_20dc5445d754` / `delivery_cff276509686` addressed. L2 CI timeout not changed.

Heavy DB gate not run. Native extraction is not product-accepted.
