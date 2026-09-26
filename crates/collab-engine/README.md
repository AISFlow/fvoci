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
set), `snapshot` (completeV1 including pending/delete set), `inspect`,
`project` (read-only Tiptap JSON of the `prosemirror` fragment; optional
`content_json` on `ok`; never overloads `update_b64`). `project` counts toward
`max_ops` and does not set `mutated`. JSON is capped at 1 MiB
(`DOCUMENT_MAX_BODY_BYTES`); depth 128 / 100k nodes / per-string 1 MiB.
Nested Any arrays/maps (including numeric/bool leaves) count toward the node
and depth caps before allocation. Output is size-checked with a counting
serializer, not a full `to_vec` then reject. Source `withoutYChange` keeps
empty `marks: []` and does not rewrite surviving marks (nested `ychange` in
retained mark attrs stays).
`seed_from_tiptap` (stateless; `content_json` is Tiptap JSON *text*, ≤ 1 MiB)
returns in `update_b64` the updateV1 of a fresh Doc seeded like source
`tiptapJsonToYUpdate` (editor schema defaults, `null`/`ychange` attrs dropped,
marks as text format attributes). Compared as decoded trees against the TS
oracle in `compat/fixtures/yjs-seed` (`tests/seed_compat.rs`).
Yjs reserves the text attribute name `ychange`; fixtures `ychange_only.v1`
and `ychange_retained_nested.v1` use hashed `ychange--xxxxxxxx` plus a
surviving mark whose attrs contain `ychange`. Unsupported CRDT shape →
`malformed` (including a non-XML child such as `Y.Map` or an Any embed at
fragment or element; yrs `XmlNodes` would otherwise truncate). Over-limit →
`resource_limit`. `encoding != 1` → `unsupported/encoding_v2`.

Mark arrays are **not** exact raw JS JSON. y-tiptap emits marks in Y.Text
format-item order; that order is not exposed by yrs 0.28, so Project emits
marks sorted by raw attribute name. ProseMirror `Node.fromJSON` re-ranks
marks on load; typed mark contents and schema-ranked editor state match the
JS oracle. Fixtures keep `js_raw_prosemirror_json` separately and pin
`project_prosemirror_json` (raw-key sort). `serde_json` is built without
`preserve_order`, so object keys are sorted (JS keeps insertion order).
Source `yDocToTiptapJson` falls back to `{type:"doc",content:[]}` when
`!isTiptapDoc(json)`; y-tiptap always returns `type:"doc"` with an array, so
that fallback is unreachable and Rust has no equivalent.

Intentional non-Tiptap-client divergences (not byte-equal with JS):
`Any::Number` may print `1e21` vs JS `1e+21`; Rust returns `malformed` for
non-finite numbers (JS `null`) and prints i64 (JS bigint64 can throw).
`Any::Undefined` inside `Any::Array` is `malformed` (JS `JSON.stringify`
yields `null`). `Any::Buffer` attrs are `malformed` (JS index-keyed object).
Element attr `undefined` omits the key (JS may emit `"attrs":{}`).

Response `EngineReport.outcome`: `ok` (`applied`, `pending`, **`durable: false`**,
optional `content_json`),
`malformed`, `unsupported`, `resource_limit`, `worker_failure`.
`applied` is this child's Doc only. Parent FIFO/DB; broadcast after durable
commit. Denied/uncertain → `kill_and_reap` and reload committed bytes. Never
Yrs-undo as DB rollback.

Caps: `max_input_bytes == max_output_bytes` (default 8 MiB =
`STATE_OVERSIZE_FACTOR * DOCUMENT_MAX_BODY_BYTES`) so snapshots reload;
`max_load_bytes` (32 MiB) is the decoded snapshot+tail aggregate and is
checked **together with each blob** before base64 copies; JSON frames 48 MiB;
tail rows 64; global live children 8 with immediate `ResourceLimit` (not a wait);
per-document uniqueness is the future room map. Native `RLIMIT_AS` is 1 GiB
and parent-observed RSS kill is 512 MiB, so eight live children budget 4 GiB
RSS. A 32 MiB aggregate load is in range for representative fragmented
snapshot+tail data; structurally memory-heavy CRDTs still return
`ResourceLimit` (Memory/Output) and are not a universal decode guarantee.
Apply succeeds only when the authoritative completeV1 (pending + delete set)
still fits the 8 MiB reload cap; the apply reply is small `applied`/`pending`
metadata, and `snapshot` supplies the bytes at persist points. Oversize
recycles the child before any parent DB admission. `load` is once per child
session and is refused after a successful `apply` so two documents cannot
merge; `ping` before the first load is allowed. Direct `apply` onto an empty
child remains valid. Child
`env_clear` / scrub; no inherited `DATABASE_APP_URL`. Per-request wall
deadline stays 8 s. Cumulative child `RLIMIT_CPU` is
`ceil(timeout_ms/1000) * max_ops` so 256 healthy ops are not killed by an
8 s process CPU budget. Writer and reader run on helper threads; timeout
kills, waits, and joins. Pipe EOF/IO waits for an observed exit until that
same deadline; a complete frame already delivered is kept even if the child
then exits.

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
  the bound, not a UB sandbox. Nested Any / stack overflow may surface as
  `ResourceLimit{Stack}` (including Rust abort with `overflowed its stack`)
  or `ChildCrash`; they must not be reported as a protocol EOF.
- If Yjs snapshot bytes fail `Snapshot::decode_v1`, tests panic with the
  fixture path instead of vendoring a parser.
