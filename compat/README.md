# Compat probe (not product)

Isolated evidence under `compat/**` only. Product code must not import this tree or call Node.

Source inspected locally (read-only) at `393795261322b916e588043cf94feca999175843`. Target base `31b6790782d54eca3603d92ebac77a7b1e51aca2`. This is a risk probe, not a CRDT implementation and not Hocuspocus-provider compatibility.

## Pins

| Piece | Version |
| --- | --- |
| rustc / cargo | 1.98.1 (`CARGO_HOME=/home/kinesis/orca/toolchains/fvoci-rust/cargo`) |
| yrs | 0.23.5 (`skip_gc`, update V1) |
| yjs | 13.6.32 |
| y-protocols | 1.0.7 |
| @hocuspocus/provider + common | 4.6.0 |
| @tiptap/core, starter-kit, pm | 3.31.3 |
| @tiptap/y-tiptap | 3.0.9 |
| Y.Doc | `gc: false` / Yrs `skip_gc: true` |
| XML fragment | `prosemirror` |
| encoding | updateV1 (`Y.encodeStateAsUpdate` / `encode_state_as_update_v1`) |

JS deps are dev-only (`compat/js`). `node_modules/` and `target/` are gitignored.

## Commands (cwd `compat/`)

```bash
# Use the checked-in fixtures. Optional synthetic specimens go elsewhere:
# python3 fixtures/gen.py --output-dir /tmp/fvoci-synthetic-fixtures
npm --prefix js ci --ignore-scripts
export CARGO_HOME=/home/kinesis/orca/toolchains/fvoci-rust/cargo
export RUSTUP_HOME=/home/kinesis/orca/toolchains/fvoci-rust/rustup
export PATH="$CARGO_HOME/bin:$PATH"
export CARGO_TARGET_DIR="$PWD/target"
cargo build --locked --bins
YRS_BRIDGE="$PWD/target/debug/yrs-bridge" node js/probe.mjs
node js/hocuspocus-handshake.mjs
./target/debug/extract-probe fixtures/sample.pdf fixtures/sample.docx fixtures/sample.hwpx fixtures/sample.hwp
```

After preparing dependencies, `bash run.sh` builds with `--locked --offline` and bounds each Node probe to 30 seconds. It uses the caller's Rust environment; the exports above describe this session's local toolchain only.

## Results (this worktree)

| Check | Exit | Wall | What happened |
| --- | --- | --- | --- |
| `cargo build --bins` (after deps) | 0 | 0.36s / warm 0.29s | `yrs-bridge`, `extract-probe` |
| `node js/probe.mjs` | 0 | 0.184s | All 6 cases pass |
| `node js/hocuspocus-handshake.mjs` | 0 | 0.473s | 3 real provider frames; raw y-protocols decode **fails** |
| `extract-probe` on four fixtures | 0 | 0.007s | PDF/DOCX/HWPX tokens; HWP CFB recognized, body parser **absent** |
| `yrs-bridge` ping | 0 | 0.007s | `skip_gc`, fragment `prosemirror`, `updateV1` |

First `cargo build` also downloaded crates (~10s) then failed until `XmlFragment`/`GetString` traits were imported; not hidden.

### Yjs stored updates through Yrs

`probe.mjs` uses Tiptap starter-kit + y-tiptap to write fragment `prosemirror`, then pipes updateV1 through `yrs-bridge`:

- Korean roundtrip: `안녕`
- Concurrent clients (IDs 11/22): `가나다` and `🚀✨` both present after Yrs merge
- Duplicate + out-of-order incremental updates converge with in-order Yjs
- State-vector diff reconnect: offline `클라한글` + server `서버emoji🎉` + `base`
- Persist encode_state_v1 bytes, load into a fresh Yrs doc, subsequent `after-persist` edit works (125-byte snapshot in this run)

This is **document-update** interchange only.

### Hocuspocus provider vs y-protocols sync

Real `@hocuspocus/provider@4.6.0` handshake against a capturing WebSocket (no network server):

1. **Auth** (64B): varString document name, type 2, `writeAuthentication` token, version `4.6.0`
2. **Sync** (50B): name + type 0 + inner y-protocols SyncStep1 (`00 01 00`)
3. **Awareness** (58B): name + type 1 + awareness blob

`y-protocols/sync.readSyncMessage` on the **raw** frame: `Unknown message type` (first varUint is 45 = length of the document name, not 0/1/2). After stripping name + Hocuspocus type, the Sync **payload** decodes as SyncStep1. Yrs `apply_update` is not this adapter.

Missing adapter (exact):

1. lib0 **varString document name** (and FVOCI `sessionAwareness` routing key if enabled)
2. Provider **MessageType** dispatch: Sync=0, Awareness=1, Auth=2, QueryAwareness=3, Stateless=5, CLOSE=7, SyncStatus=8, Ping=9, Pong=10
3. **Auth**: `AuthMessageType.Token` + token string + provider version; FVOCI also authenticates `fvoci_session` on the WebSocket upgrade (not in Y sync)
4. **Stateless** JSON control (`persist` / `persisted` / `persist-failed`)
5. Awareness/update payloads still wrapped; inner sync/update bytes are y-protocols, the envelope is not

Do not treat Yrs as a Hocuspocus server.

### Attachment extract / thumbnail

Inspected locally (see `fixtures/NOTICE.md`): source `pickExtractor` sends `.hwp`/`.hwpx` to `hwpx-js`, pdf/docx/… to `officeparser` (pdfjs worker must be `file:`), thumbnails only raster MIME via magick-wasm. PDF/DOCX/HWP/HWPX are not in that thumbnail set.

PDF/DOCX are generated representatives. HWP/HWPX are owner-authored Hancom
samples containing `안녕`, replaced at `bea3243`; see `fixtures/NOTICE.md`.
They are real containers, not renamed text. The generator only writes to a
separate empty output directory.

| Format | This probe | Native on host | Blocker |
| --- | --- | --- | --- |
| PDF | uncompressed `(compat probe)` string | no pdftotext/mutool/poppler | production path is officeparser+pdfjs, not this scanner |
| DOCX | `w:t` → `안녕 compat 🚀` | no LibreOffice | production path is officeparser |
| HWPX | ZIP `hp:t` → `안녕` | no hwpx CLI | production path is hwpx-js |
| HWP | authored HWP 5.0, BodyText present; probe reads CFB magic + `HWP Document File` | no hwp5proc/olefile | **no BodyText decoder** |
| Thumbnails | not run | no ImageMagick | magick-wasm + image MIME only |

No conversion framework was added.

## Untested (not pass)

- Two real FVOCI UI clients
- `fvoci_session` / `onAuthenticate` / permission revoke / clientId camping
- Collab process restart + reconnect through `/collab`
- `@hocuspocus/extension-redis` patched path
- Full Tiptap schema (mention/embed/callout/attachment UniqueID) vs starter-kit
- Production extractors (`hwpx-js`, officeparser, pdfjs, magick-wasm)

## Remaining

Wire a Hocuspocus frame mux in front of Yrs if the Rust collab socket must speak the existing web client. Keep stored state as gc:false updateV1. Attachment text for HWP needs a real HWP5 parser or a pinned native tool; raster thumbnails need magick or equivalent. Coordinator-owned product crates are untouched.
