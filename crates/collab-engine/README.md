# collab-engine

Isolated native Yrs child for FVOCI `COLLAB_STATE_ENCODING_V1` / Yjs 13.6.32
updateV1. This crate is **not** wired to `fvoci-server`, does not open
WebSockets, and does not touch a database. rlimits isolate resource/failure,
not the filesystem or network.

## Pins

| Item | Value |
| --- | --- |
| yrs | `=0.28.0` checksum `52c70dc8beca8666c77612a96889106ca3cd65318609721f464624ff79685da9` feature `small-client` |
| Yjs (fixtures only) | 13.6.32 |
| Doc | `skip_gc=true`, `OffsetKind::Utf16` |
| fragment | `prosemirror` |
| Platforms | Linux x86_64 and aarch64. Other OS refuse closed. |

Parent adapters depend with `default-features = false` (process + protocol only).
Feature `worker` compiles Yrs and the helper binary (`required-features`).
Tests: `cargo test --locked --offline --features worker -j 2`.

## Protocol (frozen)

Length-prefixed frames: `u32 LE` + JSON. One in-flight op per `EngineSession`.

Requests: `ping`, `load` (committed snapshot + tail), `apply` (candidate),
`sync` (state vector → `encode_state_as_update_v1` including pending/delete
set), `snapshot` (completeV1 including pending/delete set), `inspect`.
`encoding != 1` → `unsupported/encoding_v2`.

Response `EngineReport.outcome`: `ok` (`applied`, `pending`, **`durable: false`**),
`malformed`, `unsupported`, `resource_limit`, `worker_failure`.
`applied` is this child's Doc only. Parent FIFO/DB; broadcast after durable
commit. Denied/uncertain → `kill_and_reap` and reload committed bytes. Never
Yrs-undo as DB rollback.

Caps: `max_input_bytes == max_output_bytes` (default 8 MiB =
`STATE_OVERSIZE_FACTOR * DOCUMENT_MAX_BODY_BYTES`) so snapshots reload;
`max_load_bytes` (32 MiB) is the decoded snapshot+tail aggregate and is
checked **together with each blob** before base64 copies; JSON frames 48 MiB;
tail rows 64; global live children 8 with immediate `ResourceLimit` (not a wait);
per-document uniqueness is the future room map. Apply succeeds only when the
authoritative completeV1 (pending + delete set) still fits the 8 MiB reload
cap; oversize recycles the child before any parent DB admission. Child
`env_clear` / scrub; no inherited `DATABASE_APP_URL`. Deadline covers
preflight+serialize+write+read. Writer and reader run on helper threads;
timeout kills, waits, and joins.

## Commands

```sh
export CARGO_HOME=/home/kinesis/orca/toolchains/fvoci-rust/cargo
export RUSTUP_HOME=/home/kinesis/orca/toolchains/fvoci-rust/rustup
export PATH="$CARGO_HOME/bin:$PATH"
cd crates/collab-engine
export CARGO_TARGET_DIR="$PWD/target"
cargo fetch --locked
cargo test --locked --offline --features worker -j 2
cargo test --locked --offline --features worker,test-hang -j 2
cargo clippy --locked --offline --all-targets --features worker,test-hang -- -D warnings
rustfmt --edition 2021 src/*.rs src/bin/*.rs tests/*.rs
```

Fixture generator (dev only, never in product/tests):

```sh
npm --prefix js ci --ignore-scripts --no-audit --no-fund
node js/generate.mjs
```

## Honest remaining

- Not a Hocuspocus adapter. Not product `/collab`.
- 0.28.0 still has no decode recursion/remaining-input cap; child rlimits are
  the bound, not a UB sandbox.
- If Yjs snapshot bytes fail `Snapshot::decode_v1`, tests panic with the
  fixture path instead of vendoring a parser.
