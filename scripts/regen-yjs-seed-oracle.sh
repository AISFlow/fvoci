#!/usr/bin/env bash
# Regenerates compat/fixtures/yjs-seed/oracle/ from the Node document convert
# helper (the TS `tiptapJsonToYUpdate` oracle; dev/test only):
#   md-<name>.yupdate  base64 updateV1 of compat/fixtures/markdown-oracle/<name>.json
#   <case>.yupdate     base64 updateV1 of compat/fixtures/yjs-seed/cases/<case>.json
#   <case>.error       helper refusal code when the TS side throws
#   ../schema.json     editor schema attrs/defaults/mark overlap
# Requires FVOCI_DOCUMENT_CONVERT_BIN (see scripts/prepare-document-convert.sh).
# Client-side check of the Rust seed (Yjs + y-tiptap + editor schema), dev only:
#   node --import scripts/document-convert/node_modules/tsx/dist/loader.mjs \
#     scripts/document-convert/seed-client-check.mjs <collab-engine> <case.json>...
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/compat/fixtures/yjs-seed"
OUT="$DIR/oracle"
BIN="${FVOCI_DOCUMENT_CONVERT_BIN:?set FVOCI_DOCUMENT_CONVERT_BIN (scripts/prepare-document-convert.sh)}"

rm -rf "$OUT"
mkdir -p "$OUT"

seed() {
  local input="$1" name="$2" resp
  resp="$(jq -c '{op: "tiptap_to_yjs_update", contentJson: .}' "$input" | "$BIN")"
  if [[ "$(jq -r '.ok' <<<"$resp")" == "true" ]]; then
    jq -j '.updateB64' <<<"$resp" >"$OUT/$name.yupdate"
  else
    jq -j '.code' <<<"$resp" >"$OUT/$name.error"
  fi
}

for f in "$ROOT"/compat/fixtures/markdown-oracle/*.json; do
  seed "$f" "md-$(basename "${f%.json}")"
done
for f in "$DIR"/cases/*.json; do
  seed "$f" "$(basename "${f%.json}")"
done
node --import "$ROOT/scripts/document-convert/node_modules/tsx/dist/loader.mjs" \
  "$ROOT/scripts/document-convert/schema-dump.mjs" >"$DIR/schema.json"
echo "regenerated $(ls "$OUT" | wc -l) yjs seed oracle cases in $OUT"
