#!/usr/bin/env bash
# Fails when the editor schema (getSchema(createFvociExtensions())) no longer
# matches tests/fixtures/yjs-seed/schema.json, the table the Rust seed writer
# (crates/collab-engine/src/seed.rs) is checked against. Fix by updating the
# Rust schema table and running scripts/regen-yjs-seed-oracle.sh.
# Needs the web workspace's dependencies (`bun ci`; dev/CI only).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE="$ROOT/tests/fixtures/yjs-seed/schema.json"
[[ -f "$FIXTURE" ]] || { echo "missing $FIXTURE" >&2; exit 2; }
[[ -d "$ROOT/node_modules/@fvoci/editor" ]] || {
  echo "missing the @fvoci/editor workspace link; run bun ci" >&2
  exit 2
}

current="$(mktemp "${TMPDIR:-/tmp}/yjs-seed-schema.XXXXXX")"
trap 'rm -f "$current"' EXIT
bun "$ROOT/scripts/document-convert/schema-dump.mjs" >"$current"
if ! diff -u "$FIXTURE" "$current"; then
  echo "editor schema drifted from tests/fixtures/yjs-seed/schema.json (see diff above)" >&2
  exit 1
fi
echo "yjs seed schema matches the editor schema"
