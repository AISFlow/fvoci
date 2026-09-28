#!/usr/bin/env bash
# Create the GitHub pre-release for an existing tag from a rendered release
# directory (scripts/release-dist.sh). A re-run is a no-op when the tag
# already has a complete release for the same image digest; it never replaces
# assets and fails on any other existing release.
# Needs GH_TOKEN (contents: write) and GITHUB_REPOSITORY; the image tag
# 0.y.z must already point at the recorded index digest.
#
#   scripts/release-publish.sh --tag v0.y.z --dist DIR
set -euo pipefail

TAG="" DIST=""
while (($#)); do
  case "$1" in
    --tag) TAG="$2" ;;
    --dist) DIST="$2" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift 2
done
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fail() { echo "release-publish: $*" >&2; exit 1; }
ASSETS=(compose.yml SHA256SUMS release.json RELEASE-NOTES.md)

(cd "$DIST" && sha256sum --strict -c SHA256SUMS >/dev/null) || fail "SHA256SUMS does not match $DIST"
VERSION="$(python3 -c 'import json,sys; r=json.load(open(sys.argv[1])); assert "v"+r["version"]==sys.argv[2], r; print(r["version"])' "$DIST/release.json" "$TAG")"
DIGEST="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["indexDigest"])' "$DIST/release.json")"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# Release state from the API (drafts included for this contents: write token),
# not from gh error text.
STATE="$(python3 "$ROOT/scripts/release-api.py" release-state --tag "$TAG")"
case "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["state"])' "$STATE")" in
  none) ;;
  published)
    gh release download "$TAG" --repo "$GITHUB_REPOSITORY" --pattern release.json --dir "$WORK" \
      || fail "release $TAG exists without release.json; inspect and delete it by hand"
    RECORDED="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["indexDigest"])' "$WORK/release.json")"
    [[ "$RECORDED" == "$DIGEST" ]] || fail "release $TAG already records $RECORDED, not $DIGEST; refusing to overwrite"
    python3 - "$STATE" "${ASSETS[@]}" <<'PY' || fail "release $TAG for $DIGEST lacks assets; delete the partial release by hand and re-run"
import json, sys
missing = sorted(set(sys.argv[2:]) - set(json.loads(sys.argv[1])["assets"]))
sys.exit(f"missing {missing}" if missing else 0)
PY
    echo "release $TAG already published for $DIGEST; nothing to do"
    exit 0
    ;;
  *) fail "release $TAG is a draft (an interrupted upload); delete it by hand and re-run" ;;
esac

# The publish job tagged the index only after both smokes passed; the release
# names exactly that tag (anonymous read: the package is public by now).
IMAGE_REF="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["image"])' "$DIST/release.json")"
TAGGED="$(env -u REGISTRY_USER -u REGISTRY_PASSWORD python3 "$ROOT/scripts/release-api.py" registry-digest \
  --image "${IMAGE_REF%%:*}" --tag "$VERSION")"
[[ "$TAGGED" == "$DIGEST" ]] || fail "${IMAGE_REF%%:*}:$VERSION is '${TAGGED:-missing}', not $DIGEST; run the publish job first"

# --verify-tag: publish only for a git tag that already exists on the remote;
# the workflow never creates or moves git tags.
gh release create "$TAG" --repo "$GITHUB_REPOSITORY" --verify-tag --prerelease --latest=false \
  --title "FVOCI ${VERSION} (trial)" --notes-file "$DIST/RELEASE-NOTES.md" \
  "${ASSETS[@]/#/$DIST/}"

mkdir -p "$WORK/published"
gh release download "$TAG" --repo "$GITHUB_REPOSITORY" --dir "$WORK/published"
(cd "$WORK/published" && sha256sum --strict -c SHA256SUMS) || fail "published assets do not match SHA256SUMS"
cmp -s "$WORK/published/SHA256SUMS" "$DIST/SHA256SUMS" || fail "published SHA256SUMS differs from the rendered one"
echo "published pre-release $TAG for $DIGEST"
