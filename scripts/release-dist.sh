#!/usr/bin/env bash
# Render the release files for one published image index (docs/RELEASING.md):
#   compose.yml    the user compose with every ${FVOCI_IMAGE...} reference
#                  replaced by the digest-pinned image; never committed back
#   release.json   the release record (version, source SHA, digests)
#   RELEASE-NOTES.md
#   SHA256SUMS     over the three files above
#
#   scripts/release-dist.sh --version 0.y.z --sha <40-hex> --repository owner/name \
#     --image ghcr.io/aisflow/fvoci --index-digest sha256:... \
#     --amd64-digest sha256:... --arm64-digest sha256:... --run-url URL --out DIR
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="" SHA="" REPOSITORY="" IMAGE="" INDEX_DIGEST="" AMD64_DIGEST="" ARM64_DIGEST="" RUN_URL="" OUT=""
while (($#)); do
  case "$1" in
    --version) VERSION="$2" ;;
    --sha) SHA="$2" ;;
    --repository) REPOSITORY="$2" ;;
    --image) IMAGE="$2" ;;
    --index-digest) INDEX_DIGEST="$2" ;;
    --amd64-digest) AMD64_DIGEST="$2" ;;
    --arm64-digest) ARM64_DIGEST="$2" ;;
    --run-url) RUN_URL="$2" ;;
    --out) OUT="$2" ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift 2
done

# Worker contract: the standalone user compose lives at the first existing path
# and names the FVOCI image only through ${FVOCI_IMAGE...}.
COMPOSE_SOURCE=""
for candidate in infra/rust/compose.user.yml infra/rust/compose.yml; do
  if [[ -f "$ROOT/$candidate" ]]; then
    COMPOSE_SOURCE="$candidate"
    break
  fi
done
if [[ -z "$COMPOSE_SOURCE" ]]; then
  echo "no user compose file found" >&2
  exit 1
fi

mkdir -p "$OUT"
export VERSION SHA REPOSITORY IMAGE INDEX_DIGEST AMD64_DIGEST ARM64_DIGEST RUN_URL OUT COMPOSE_SOURCE
python3 - "$ROOT" <<'PY'
import json, os, re, sys
from pathlib import Path

root = Path(sys.argv[1])
env = os.environ
errors = []
if not re.fullmatch(r"0\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", env["VERSION"]):
    errors.append(f"version {env['VERSION']!r} is not 0.y.z")
if not re.fullmatch(r"[0-9a-f]{40}", env["SHA"]):
    errors.append("--sha must be a full commit SHA")
if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", env["REPOSITORY"]):
    errors.append("--repository must be owner/name")
if not re.fullmatch(r"[a-z0-9.-]+(/[a-z0-9._-]+)+", env["IMAGE"]):
    errors.append("--image must be a lowercase registry/repository without tag")
for key in ("INDEX_DIGEST", "AMD64_DIGEST", "ARM64_DIGEST"):
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", env[key]):
        errors.append(f"{key.lower()} must be sha256:<64 hex>")
if not env["RUN_URL"].startswith("https://"):
    errors.append("--run-url must be an https URL")
if errors:
    sys.exit("\n".join(errors))

out = Path(env["OUT"])
image_ref = f"{env['IMAGE']}:{env['VERSION']}@{env['INDEX_DIGEST']}"

compose_text = (root / env["COMPOSE_SOURCE"]).read_text(encoding="utf-8")
pattern = re.compile(r"\$\{FVOCI_IMAGE(?:[:]?[-?][^}]*)?\}")
rendered, count = pattern.subn(image_ref, compose_text)
if count == 0:
    sys.exit(f"{env['COMPOSE_SOURCE']}: no ${{FVOCI_IMAGE...}} image reference to pin")
if "FVOCI_IMAGE" in rendered:
    sys.exit(f"{env['COMPOSE_SOURCE']}: FVOCI_IMAGE left after rendering")
image_lines = [
    line.split(":", 1)[1].strip().strip("'\"")
    for line in rendered.splitlines()
    if re.match(r"\s+image:", line)
]
fvoci = [ref for ref in image_lines if ref.startswith(env["IMAGE"] + ":") or ref.startswith(env["IMAGE"] + "@")]
if not fvoci or any(ref != image_ref for ref in fvoci):
    sys.exit(f"rendered FVOCI image references {fvoci} are not all {image_ref}")
header = (
    f"# FVOCI {env['VERSION']} ({env['SHA']}), rendered from {env['COMPOSE_SOURCE']}\n"
    f"# by the release workflow. The image is pinned by manifest digest.\n"
)
(out / "compose.yml").write_text(header + rendered, encoding="utf-8")

record = {
    "version": env["VERSION"],
    "tag": f"v{env['VERSION']}",
    "sourceSha": env["SHA"],
    "image": image_ref,
    "indexDigest": env["INDEX_DIGEST"],
    "platforms": {"linux/amd64": env["AMD64_DIGEST"], "linux/arm64": env["ARM64_DIGEST"]},
    "composeSource": env["COMPOSE_SOURCE"],
}
(out / "release.json").write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")

notes = (root / "scripts/release-notes-template.md").read_text(encoding="utf-8")
for key, value in {
    "@VERSION@": env["VERSION"],
    "@SHA@": env["SHA"],
    "@REPOSITORY@": env["REPOSITORY"],
    "@IMAGE_REF@": image_ref,
    "@AMD64_DIGEST@": env["AMD64_DIGEST"],
    "@ARM64_DIGEST@": env["ARM64_DIGEST"],
    "@RUN_URL@": env["RUN_URL"],
}.items():
    notes = notes.replace(key, value)
left = sorted(set(re.findall(r"@[A-Z0-9_]+@", notes)))
if left:
    sys.exit(f"release notes placeholders left: {left}")
(out / "RELEASE-NOTES.md").write_text(notes, encoding="utf-8")
PY

(cd "$OUT" && sha256sum compose.yml release.json RELEASE-NOTES.md >SHA256SUMS)
echo "rendered $OUT from $COMPOSE_SOURCE"
