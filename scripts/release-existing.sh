#!/usr/bin/env bash
# Idempotency guard for a release re-run. Prints the image index digest that
# this version already carries (empty when it has none yet), so the workflow
# reuses it instead of building again:
#   - a GitHub release for the tag must carry release.json for the same source
#     SHA, and the registry tag must still point at the digest it records;
#   - a registry tag without a release (a run that tagged after its smoke but
#     stopped before the release job) is reused only when its OCI labels name
#     this version and SHA.
# The :0.y.z tag is only applied after the smoke passed (docs/RELEASING.md), so
# an index pushed by digest for a run whose smoke failed is never reused.
# Decisions use exit codes and JSON (scripts/release-api.py), not error text.
# Anything else fails closed; this never moves a tag.
# Needs GH_TOKEN (contents: read), GITHUB_REPOSITORY, REGISTRY_USER/PASSWORD
# (packages: read) and a docker registry login for the label check.
#
#   scripts/release-existing.sh --tag v0.y.z --version 0.y.z --sha <sha> --image ghcr.io/aisflow/fvoci
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
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
API=(python3 "$ROOT/scripts/release-api.py")

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

STATE="$("${API[@]}" release-state --tag "$TAG")"
RECORD_DIGEST=""
case "$(python3 -c 'import json,sys; print(json.loads(sys.argv[1])["state"])' "$STATE")" in
  none) ;;
  published)
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
    ;;
  *) fail "release $TAG is a draft; inspect it and delete it by hand before re-running" ;;
esac

REGISTRY_DIGEST=""
set +e
REGISTRY_DIGEST="$("${API[@]}" registry-digest --image "$IMAGE" --tag "$VERSION")"
status=$?
set -e
case "$status" in
  0) ;;
  4)
    # The job token cannot read the package; the first release has no package
    # yet. Without a release there is no recorded digest to protect here, and
    # the publish job re-checks the tag with its write token before tagging.
    [[ -z "$RECORD_DIGEST" ]] || fail "release $TAG records $RECORD_DIGEST but $IMAGE is not readable"
    echo "release-existing: $IMAGE not readable with this token; treating $VERSION as untagged" >&2
    ;;
  *) fail "could not inspect $IMAGE:$VERSION" ;;
esac

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
