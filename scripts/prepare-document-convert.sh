#!/usr/bin/env bash
# Installs the locked dependencies of the document convert helper (run by Bun)
# and prints FVOCI_DOCUMENT_CONVERT_BIN=<runner>. Any failure exits non-zero.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
RUNNER="$ROOT/scripts/run-document-convert.sh"
CONVERT="$ROOT/scripts/document-convert/convert.mjs"

(cd "$ROOT" && bun ci) >&2

[[ -f "$CONVERT" ]] || { echo "missing document convert script at $CONVERT" >&2; exit 1; }
[[ -x "$RUNNER" ]] || { echo "document convert runner is not executable: $RUNNER" >&2; exit 1; }

# Smoke: one real conversion proves Bun and the editor package resolve.
if ! printf '%s' '{"op":"md_to_tiptap","markdown":"# ok"}' | "$RUNNER" | grep -q '"ok":true'; then
  echo "document convert helper smoke test failed" >&2
  exit 1
fi
echo "FVOCI_DOCUMENT_CONVERT_BIN=$RUNNER"
