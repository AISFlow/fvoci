#!/usr/bin/env bash
# Installs the locked dependencies of the Node document convert helper and
# prints FVOCI_DOCUMENT_CONVERT_BIN=<runner>. Any failure exits non-zero.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNNER="$ROOT/scripts/run-document-convert.sh"
CONVERT="$ROOT/scripts/document-convert/convert.mjs"

npm ci --prefix "$ROOT/packages/editor" --ignore-scripts --no-audit --no-fund >&2
npm ci --prefix "$ROOT/scripts/document-convert" --ignore-scripts --no-audit --no-fund >&2

[[ -f "$CONVERT" ]] || { echo "missing document convert script at $CONVERT" >&2; exit 1; }
[[ -x "$RUNNER" ]] || { echo "document convert runner is not executable: $RUNNER" >&2; exit 1; }

# Smoke: one real conversion proves node, tsx and the editor package resolve.
if ! printf '%s' '{"op":"md_to_tiptap","markdown":"# ok"}' | "$RUNNER" | grep -q '"ok":true'; then
  echo "document convert helper smoke test failed" >&2
  exit 1
fi
echo "FVOCI_DOCUMENT_CONVERT_BIN=$RUNNER"
