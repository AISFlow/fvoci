# document-extract

Native HWP 5.0 / HWPX **body** extraction for FVOCI. This crate is a compile
boundary of its own. The coordinator owns the root manifest, CI, and later
product attachment wiring. There is **no** unauthenticated upload endpoint here.
Native extraction is **not accepted** as a product feature yet.

## Pin

| Item | Value |
| --- | --- |
| Upstream | https://github.com/edwardkim/rhwp (not a similarly named fork, not npm `@rhwp/*`) |
| Rev | `e8800c8def63449808a4092798442652ed460552` (v0.8.6 line) |
| Authority | `[package.metadata.document-extract]` in `Cargo.toml` |
| crates.io `rhwp` | **absent** (404). Path checkout of that rev is the pin. |
| License | MIT, Copyright (c) 2025-2026 Edward Kim (`NOTICE-rhwp.md`) |
| Features | `default-features = false`. `native-skia` and `gpu` are **not** enabled. |
| Always-on native deps from rhwp | `wasm-bindgen` (crate root `use wasm_bindgen`; no wasm32 target), `svg2pdf`/`usvg`/`pdf-writer` on non-wasm, `image`, `rustybuzz`. Cannot strip without forking `src/lib.rs`. |

Public API used: `rhwp::parse_document`, `rhwp::parser::detect_format` /
`FileFormat` / `ParseError`, `Document.sections[].paragraphs[].text` and
`Control::Table` cells in document order. `PrvText` is never promoted to body.

GitHub reports the rhwp **repository size** near 3GB. A canceled `cargo git`
fetch materialized about 133–148MiB under shared `CARGO_HOME/git/db` before
SIGINT. This crate uses a **path** dep on a gitignored sparse checkout
(`.vendor-src/rhwp`). `fetch-rhwp.sh` does depth-1 `blob:none` **non-cone**
sparse of `src/`, `crates/`, required workspace members, and
`saved/blank2010.hwp` (needed by `include_bytes` in the library). It reads the
Cargo metadata pin, refuses dirty or mismatched trees, and does not delete them.
Do not vendor editor/samples/npm trees into product git.

`Cargo.lock` is checked in. It records the resolved crates.io graph for the path
dep (including crates.io `svg2pdf` 0.13.0). It does **not** pin `.vendor-src`
contents; `fetch-rhwp.sh` verifies the commit. rhwp's workspace
`[patch.crates-io]` is ignored for a path dep. Upstream lock pins
`edwardkim/svg2pdf` `2caeb0a038f9128b79833d803b94c2667565c4da`; reproduce that
exact rev here only if compile requires the patch.

## Limits (this process)

| Bound | Value |
| --- | --- |
| Input | 20 MiB |
| Output chars | 500_000 |
| Child timeout | 120 s default (tests use 8 s / 500 ms) |
| Child address space | Linux `RLIMIT_AS` via `pre_exec` + child `setrlimit`, same number as the RSS ceiling (default 1536 MiB). This is virtual size, not RSS. |
| Child CPU | Linux `RLIMIT_CPU` = `timeout_ms/1000` (min 1s) as backup to wall-clock kill |
| Observed RSS | parent poll; kill+reap if `VmRSS` exceeds the ceiling |
| Zip entries | 10_000 |
| Zip uncompressed sum (CD, no inflate) | 200 MiB |
| Child concurrency | 1 (slot wait is deadline-cancellable) |
| Walk nest | 8 |
| Temp files | none (stdin/stdout only; no document paths) |
| Network / embedded exec | none |
| Upstream rhwp HWP5 stream / total | 256 MiB / 512 MiB |
| Upstream rhwp HWPX XML entry | 256 MiB |

`extract_killable` is **synchronous**. The only cancel mechanism is the
`timeout_ms` deadline (and RSS/CPU rlimits). There is no external cancel token.
Dropping a `JoinHandle` that wraps this call does **not** kill the child.
Product async callers must run it off the runtime executor (or use a real
process kill path) and must not treat task cancellation as process termination.

## Outcomes

`ok` / `empty` (valid parse, no body) / `partial` (output cap, dropped
supported-scope table/header/footer/note/depth/out-of-range cell body,
**omitted failed section** with remaining valid body, **or** omitted
shape/drawing) / `unsupported` (`encrypted`, `distribution`, `drm`,
`extension_magic_mismatch`, `hwp3`, `hml`, `unknown_format`, `empty_file`) /
`corrupt` (document bytes, **or** every section dropped by the parser with no
valid body) / `resource_limit` (`input`, `zip_entries`,
`zip_uncompressed`, `output`, `time`, `memory`, `decompress`) /
`worker_failure` (missing executable, spawn/wait, child crash/signal, invalid
child JSON, rlimit apply, unsupported platform, invalid limits). SIGSEGV is a
worker crash, not a memory-limit proof. SIGABRT is `resource_limit/memory`
only when child stderr contains a Rust allocator failure (`memory allocation of
… failed`); other SIGABRT stays `worker_failure`. Shape/drawing text is still
unsupported (not walked); the warning plus omitted-shape flag makes the
outcome `partial`, not silent `ok`. Equation scripts, form caption/text, and
picture captions are walked. Hidden comments are not.

Walk order: each paragraph's text, then its controls. Table cells are
row-major. Table/picture **Top** and **Left** captions precede cells; **Bottom**
and **Right** follow cells (Hangul default caption side is Bottom). An inline
treat-as-char table is therefore after that paragraph's text, not at its
character anchor.

Pinned rhwp (`parse_sections_strict`) prefers a decoded `ViewText/Section{N}`
stream for **non-distribution** tracked-changes HWP5 when that stream exists,
decodes, and starts with `PARA_HEADER`; otherwise it uses `BodyText`.
`PrvText` is never promoted to body. A failed section record stream is replaced
upstream with `Section::default()` and only a stderr line; this crate maps that
to `partial` (recovered body remains) or `corrupt` (no recovered section).
HWP5 detection uses `raw_stream.is_none()`. HWPX detection uses an empty
paragraph list. A genuine empty HWPX from Hangul (and this crate's empty
fixture) still has ≥1 `<hp:p>`; a `<hs:sec/>` with zero paragraphs cannot be
distinguished from a drop stub at this pin and is not reported as silent
`empty`.

## Commands

Worktree-local target. Do **not** `cargo git` clone rhwp into the shared
`CARGO_HOME`. From this crate directory:

```sh
export CARGO_HOME=/home/kinesis/orca/toolchains/fvoci-rust/cargo
export RUSTUP_HOME=/home/kinesis/orca/toolchains/fvoci-rust/rustup
export PATH="$CARGO_HOME/bin:$PATH"
cd crates/document-extract
export CARGO_TARGET_DIR="$PWD/target"
sh fetch-rhwp.sh
cargo fetch --locked
# production profile: no --test-hang-ms / --dump-rlimits
cargo test --locked --offline
# hang/reap + rlimit dump tests
cargo test --locked --offline --features test-hang -- --test-threads=1
"$CARGO_TARGET_DIR/debug/document-extract" --name 안녕.hwp < fixtures/user-hancom-12.30-안녕.hwp
rustfmt --edition 2021 src/*.rs src/bin/*.rs tests/*.rs
cargo clippy --locked --offline --all-targets --features test-hang -- -D warnings
```

Default production builds omit the `test-hang` feature; `--test-hang-ms` and
`--dump-rlimits` are unknown arguments there.

Measured in this worktree (Rust 1.98.1, path rhwp already checked out).
Follow-up SHA times below; earlier first-compile numbers are historical.

| Stage | Command | Result |
| --- | --- | --- |
| crates.io fetch | `cargo fetch --locked` | 7.93s, exit 0 (prior) |
| first rhwp compile | `cargo test --locked --features test-hang` | ~45–53s compile (prior) |
| warm production tests | `cargo test --locked --offline -- --test-threads=1` | 2.34s, exit 0, 32 passed (1 unit + 24 extract + 7 boundary; hang/rlimit tests excluded) |
| warm test-hang | `cargo test --locked --offline --features test-hang -- --test-threads=1` | 2.85s, exit 0, 32 passed (1 unit + 24 extract + 7 boundary including timeout reap + RLIMIT_AS dump) |
| crate clippy | `cargo clippy --locked --offline --all-targets --features test-hang -- -D warnings` | 0.32s, exit 0 |

Those numbers are evidence of the commands, not product acceptance.

CI for this crate must run prep, then `cargo fetch --locked`, then offline
build/tests. Tests and `build.rs` must not download.

Attachment/job integration is **pending** and not this crate. This is not a
whole-FVOCI completion.

## Remaining unsupported (honest)

- HWP 3.0 / HML / DRM containers: detected, not extracted.
- Passworded documents: `encrypted`, no password API in this slice.
- Distribution documents: rejected unless a future slice defines ViewText policy.
- Text boxes inside drawing shapes: not walked; warning plus `partial` (not silent `ok`).
- Hidden comments (`tcmt`): not walked (not body).
- Product upload, search index, thumbnails: not this task.
- Compiling `rhwp` still typechecks renderer/wasm_api modules and native
  `svg2pdf`. First compile is large even with skia/gpu off.
- HWPX `<hs:sec/>` with zero `<hp:p>` cannot be distinguished from a parser
  drop at pin `e8800c8`; it is `corrupt`, not genuine `empty`.

## Fixtures

- `fixtures/user-hancom-12.30-안녕.{hwp,hwpx}`: user-authored Hangul 12.30
  documents whose body is `안녕`. See `fixtures/NOTICE.md`.
- In-process generators in `src/gen.rs`: multi-section, table, Korean/emoji,
  empty, truncated, **Section1 record-header truncation**, **HWPX malformed
  section**, table caption (Top/Bottom), equation/form/picture caption,
  corrupt, extension mismatch, zip bomb CD, path escape,
  zip entry-count, nested tables, two-line output-limit, out-of-range cells,
  and shape+body. Expected strings are declared next to the generators, not
  inferred from rhwp.
