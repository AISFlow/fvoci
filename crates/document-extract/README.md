# document-extract

Native HWP 5.0 / HWPX **body** extraction for FVOCI. This crate is a compile
boundary of its own. The coordinator owns the root manifest, CI, and later
product attachment wiring. There is **no** unauthenticated upload endpoint here.

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
fetch of that repo materialized about 133–148MiB under the shared
`CARGO_HOME/git/db` before SIGINT; that was not a completed 3GB clone. This
crate uses a **path** dep on a gitignored sparse checkout (`.vendor-src/rhwp`,
`src`+`crates` only, ~43MiB). Reproduce with `sh fetch-rhwp.sh` (depth-1,
`blob:none`, cone sparse). The script verifies origin/HEAD and refuses a dirty
or mismatched existing tree instead of overwriting it. Do not vendor the
editor/samples/npm trees into product git.

`Cargo.lock` (when generated) records resolved crates.io packages for the path
graph. It does **not** pin `.vendor-src` contents; `fetch-rhwp.sh` verifies the
commit. rhwp's workspace `[patch.crates-io]` for `svg2pdf` is ignored on a path
dep. Upstream lock pins `edwardkim/svg2pdf` commit
`2caeb0a038f9128b79833d803b94c2667565c4da`. Reproduce that exact rev at this
crate root only if native compile requires the patch (no floating branch).

## Limits (this process)

| Bound | Value |
| --- | --- |
| Input | 20 MiB |
| Output chars | 500_000 |
| Child timeout | 120 s default (tests use 8 s / 500 ms) |
| Child RSS | 1536 MiB observed; Linux `RLIMIT_AS` set to the same ceiling (address space, not RSS) |
| Child CPU | Linux `RLIMIT_CPU` = `timeout_ms/1000` (min 1s) as backup to wall-clock kill |
| Zip entries | 10_000 |
| Zip uncompressed sum (CD, no inflate) | 200 MiB |
| Child concurrency | 1 (slot wait is deadline-cancellable) |
| Walk nest | 8 |
| Temp files | none (stdin/stdout only; no document paths) |
| Network / embedded exec | none |
| Upstream rhwp HWP5 stream / total | 256 MiB / 512 MiB |
| Upstream rhwp HWPX XML entry | 256 MiB |

The parent forwards every limit as CLI flags, applies `prlimit` on Linux, then
kills+reaps on wall-clock or RSS. Child crash (nonzero exit / unexpected signal)
is `corrupt` with a `child_crash:` detail, distinct from parse corruption.

## Outcomes

`ok` / `empty` (valid parse, no body) / `partial` (output cap or walk drop) /
`unsupported` (`encrypted`, `distribution`, `drm`, `extension_magic_mismatch`,
`hwp3`, `hml`, `unknown_format`, `empty_file`) / `corrupt` (document bytes) /
`resource_limit` (`input`, `zip_entries`, `zip_uncompressed`, `output`, `time`,
`memory`, `decompress`) / `worker_failure` (missing executable, spawn/wait,
child crash/signal, invalid child JSON, rlimit apply, unsupported platform,
invalid limits). SIGSEGV is a worker crash, not a memory-limit proof.

## Commands

Worktree-local target. Do **not** `cargo git` clone rhwp into the shared
`CARGO_HOME`. After `sh fetch-rhwp.sh` and an explicit crates.io fetch of this
crate's resolved graph:

```sh
export CARGO_HOME=/home/kinesis/orca/toolchains/fvoci-rust/cargo
export RUSTUP_HOME=/home/kinesis/orca/toolchains/fvoci-rust/rustup
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_TARGET_DIR="$PWD/target"
cd crates/document-extract
sh fetch-rhwp.sh
cargo fetch --locked
cargo test --locked --offline --features test-hang
./target/debug/document-extract --name 안녕.hwp < fixtures/user-hancom-12.30-안녕.hwp
```

CI for this crate must run that prep, then `cargo fetch --locked`, then offline
build/tests. Tests and `build.rs` must not download.

`--features test-hang` is required only for the killable hang test. Default
builds do not accept `--test-hang-ms`.

Native library + `document-extract` child were compiled and tested in this
worktree (`cargo test --locked --features test-hang`: 25 passed). Attachment/job
integration is **pending** and not this crate. This is not a whole-FVOCI
completion.

## Remaining unsupported (honest)

- HWP 3.0 / HML / DRM containers: detected, not extracted.
- Passworded documents: `encrypted`, no password API in this slice.
- Distribution documents: rejected unless a future slice defines ViewText policy.
- Text boxes inside drawing shapes: not walked (tables/header/footer/notes are).
- Product upload, search index, thumbnails: not this task.
- Compiling `rhwp` still typechecks renderer/wasm_api modules and native
  `svg2pdf`. First compile is large even with skia/gpu off.

## Fixtures

- `fixtures/user-hancom-12.30-안녕.{hwp,hwpx}`: user-authored Hangul 12.30
  documents whose body is `안녕`. See `fixtures/NOTICE.md`.
- In-process generators in `src/gen.rs`: multi-section, table, Korean/emoji,
  empty, truncated, corrupt, extension mismatch, zip bomb CD, path escape.
  Expected strings are declared next to the generators, not inferred from rhwp.
