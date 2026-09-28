#!/usr/bin/env bash
# Idempotency guard for a release re-run. Prints the image index digest that
# this version already has (empty when nothing was published yet), so the
# workflow reuses it instead of building and retagging:
#   - a GitHub release for the tag must carry release.json for the same source
#     SHA, and the registry tag must still point at the digest it records;
#   - a registry tag without a release (a run that stopped before the release
#     job) is reused only when its OCI labels name this version and SHA.
# Anything else fails closed; this never moves a tag a release records.
# Needs GH_TOKEN (contents: read), GITHUB_REPOSITORY and a registry login.
#
#   scripts/release-existing.sh --tag v0.y.z --version 0.y.z --sha <sha> --image ghcr.io/aisflow/fvoci
set -euo pipefail

TAG="" VERSION="" SHA="" IMAGE=""
while (($#)); do
  case "$1" in
    --tag) TAG="$2" ;;
    --version) VERSION="$2" ;;
    --sha) SHA="$2" ;;
    --image) IMAGE="$2" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift 2
done
: "${GITHUB_REPOSITORY:?GITHUB_REPOSITORY is required}"
fail() { echo "release-existing: $*" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

RECORD_DIGEST=""
if gh release view "$TAG" --repo "$GITHUB_REPOSITORY" --json tagName >/dev/null 2>"$WORK/view.err"; then
  gh release download "$TAG" --repo "$GITHUB_REPOSITORY" --pattern release.json --dir "$WORK" \
    || fail "release $TAG exists without release.json; inspect it and delete it by hand before re-running"
  RECORD_DIGEST="$(python3 - "$WORK/release.json" "$VERSION" "$SHA" <<'PY'
import json, sys
record = json.load(open(sys.argv[1]))
if record.get("version") != sys.argv[2] or record.get("sourceSha") != sys.argv[3]:
    sys.exit(f"existing release records {record.get('version')} at {record.get('sourceSha')}, not {sys.argv[2]} at {sys.argv[3]}")
print(record["indexDigest"])
PY
)"
elif ! grep -qi 'release not found' "$WORK/view.err"; then
  cat "$WORK/view.err" >&2
  fail "could not read the GitHub release for $TAG"
fi

REGISTRY_DIGEST=""
if docker buildx imagetools inspect "$IMAGE:$VERSION" --format '{{json .Manifest}}' >"$WORK/manifest.json" 2>"$WORK/manifest.err"; then
  REGISTRY_DIGEST="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["digest"])' "$WORK/manifest.json")"
elif grep -Eqi 'not found|manifest unknown|name unknown' "$WORK/manifest.err"; then
  :
elif [[ -z "$RECORD_DIGEST" ]] && grep -qi 'denied' "$WORK/manifest.err"; then
  # GHCR answers "denied" for a package that does not exist yet (first
  # release). Without a release there is no recorded digest to protect.
  echo "release-existing: $IMAGE:$VERSION not readable (denied); treating as unpublished" >&2
else
  cat "$WORK/manifest.err" >&2
  fail "could not inspect $IMAGE:$VERSION"
fi

if [[ -n "$RECORD_DIGEST" && "$RECORD_DIGEST" != "$REGISTRY_DIGEST" ]]; then
  fail "release $TAG records $RECORD_DIGEST but $IMAGE:$VERSION is '${REGISTRY_DIGEST:-missing}'"
fi

if [[ -n "$REGISTRY_DIGEST" ]]; then
  docker buildx imagetools inspect "$IMAGE@$REGISTRY_DIGEST" --format '{{json .Image}}' >"$WORK/image.json"
  python3 - "$WORK/image.json" "$VERSION" "$SHA" <<'PY' || fail "$IMAGE:$VERSION was not built from $SHA; refusing to reuse or overwrite it"
import json, sys
images = json.load(open(sys.argv[1]))
assert set(images) == {"linux/amd64", "linux/arm64"}, sorted(images)
for platform, image in images.items():
    labels = image.get("config", {}).get("Labels") or {}
    assert labels.get("org.opencontainers.image.version") == sys.argv[2], (platform, labels)
    assert labels.get("org.opencontainers.image.revision") == sys.argv[3], (platform, labels)
PY
fi
printf '%s\n' "$REGISTRY_DIGEST"
