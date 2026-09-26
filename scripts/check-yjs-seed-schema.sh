#!/usr/bin/env bash
# Fails when the editor schema (getSchema(createFvociExtensions())) no longer
# matches compat/fixtures/yjs-seed/schema.json, the table the Rust seed writer
# (crates/collab-engine/src/seed.rs) is checked against. Fix by updating the
# Rust schema table and running scripts/regen-yjs-seed-oracle.sh.
# Needs editor and scripts/document-convert npm dependencies (dev/CI only).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FIXTURE="$ROOT/compat/fixtures/yjs-seed/schema.json"
LOADER="$ROOT/scripts/document-convert/node_modules/tsx/dist/loader.mjs"
[[ -f "$LOADER" ]] || {
  echo "missing $LOADER; run npm ci --prefix packages/editor and npm ci --prefix scripts/document-convert" >&2
  exit 2
}

current="$(mktemp "${TMPDIR:-/tmp}/yjs-seed-schema.XXXXXX")"
trap 'rm -f "$current"' EXIT
node --import "$LOADER" "$ROOT/scripts/document-convert/schema-dump.mjs" >"$current"
if ! diff -u "$FIXTURE" "$current"; then
  echo "editor schema drifted from compat/fixtures/yjs-seed/schema.json (see diff above)" >&2
  exit 1
fi
echo "yjs seed schema matches the editor schema"
