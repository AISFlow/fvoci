#!/usr/bin/env bash
# Regenerates compat/fixtures/markdown-oracle/*.{json,roundtrip.md,html} from the
# Node document convert helper (the TS `@fvoci/editor` oracle; dev/test only):
#   <name>.json          mdToTiptapJson(<name>.md)
#   <name>.roundtrip.md  tiptapDocToMd(mdToTiptapJson(<name>.md))
#   <name>.html          tiptapDocToSafeHtml(mdToTiptapJson(<name>.md))
# Requires FVOCI_DOCUMENT_CONVERT_BIN (see scripts/prepare-document-convert.sh).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DIR="$ROOT/compat/fixtures/markdown-oracle"
BIN="${FVOCI_DOCUMENT_CONVERT_BIN:?set FVOCI_DOCUMENT_CONVERT_BIN (scripts/prepare-document-convert.sh)}"

call() {
  local out
  out="$("$BIN")"
  if [[ "$(jq -r '.ok' <<<"$out")" != "true" ]]; then
    echo "convert helper failed: $out" >&2
    return 1
  fi
  printf '%s' "$out"
}

for md in "$DIR"/*.md; do
  case "$md" in *.roundtrip.md) continue ;; esac
  base="${md%.md}"
  resp="$(jq -Rs '{op: "md_to_tiptap", markdown: .}' <"$md" | call)"
  jq -S '.contentJson' <<<"$resp" >"$base.json"
  jq '{op: "tiptap_to_md", contentJson: .}' "$base.json" | call | jq -j '.markdown' >"$base.roundtrip.md"
  jq -Rs '{op: "md_to_safe_html", markdown: .}' <"$md" | call | jq -j '.html' >"$base.html"
done
echo "regenerated $(ls "$DIR"/*.json | wc -l) markdown oracle cases in $DIR"
