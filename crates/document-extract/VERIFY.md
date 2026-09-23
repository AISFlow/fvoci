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

Logs: `/tmp/fvoci-extract-test-default.log`, `/tmp/fvoci-extract-test-hang.log`, `/tmp/fvoci-extract-clippy.log`.

B1: HWP5 `raw_stream.is_none()` and HWPX empty paragraph list map parser `Section::default()` drops to `Partial` (recovered body) or `Corrupt` (none). Genuine empty fixtures still `Empty`. HWPX `<hs:sec/>` with zero `<hp:p>` cannot be distinguished from a drop at this pin.

B2: table/picture captions walked; Top/Left before row-major cells, Bottom/Right after.

M2: warnings collapsed per kind (×N), cap 32; stdout `pipe exceeded bound` classified as `resource_limit/output` before ChildCrash.

M3: SIGABRT → Memory only with `memory allocation of` + `failed` on stderr.

M4: equation script, form caption/text, picture caption extracted; HiddenComment not walked.

L1: helper `stdin().take(max_input+1)`.

Inbox `msg_20dc5445d754` / `delivery_cff276509686` addressed. L2 CI timeout not changed.

Heavy DB gate not run. Native extraction is not product-accepted.
