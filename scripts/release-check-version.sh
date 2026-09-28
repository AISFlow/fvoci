#!/usr/bin/env bash
# Release version policy: the root Cargo.toml [package] version is the single
# source. Every other place that carries the product version is derived from
# it and must match; a 0.y.z trial release refuses 1.0.0+ and pre-release
# suffixes. On success prints the version on stdout.
#
#   scripts/release-check-version.sh [--tag v0.y.z] [--image-tag 0.y.z]
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TAG=""
IMAGE_TAG=""
while (($#)); do
  case "$1" in
    --tag) TAG="${2:?--tag needs a value}"; shift 2 ;;
    --image-tag) IMAGE_TAG="${2:?--image-tag needs a value}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

TAG="$TAG" IMAGE_TAG="$IMAGE_TAG" python3 - "$ROOT" <<'PY'
import json, os, re, sys, tomllib
from pathlib import Path

root = Path(sys.argv[1])
errors = []

with (root / "Cargo.toml").open("rb") as fh:
    version = tomllib.load(fh)["package"]["version"]
if not re.fullmatch(r"0\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
    errors.append(f"Cargo.toml version {version!r} is not a 0.y.z trial version")

with (root / "Cargo.lock").open("rb") as fh:
    locked = [p["version"] for p in tomllib.load(fh)["package"] if p["name"] == "fvoci-server"]
if locked != [version]:
    errors.append(f"Cargo.lock fvoci-server {locked} != Cargo.toml {version}")

# The committed OpenAPI document is generated from the Rust DTOs
# (scripts/generate-api.sh); its info.version comes from src/api/openapi.rs.
openapi = json.loads((root / "apps/web/openapi.json").read_text(encoding="utf-8"))
if openapi["info"]["version"] != version:
    errors.append(f"apps/web/openapi.json info.version {openapi['info']['version']!r} != {version}")
source = (root / "src/api/openapi.rs").read_text(encoding="utf-8")
literal = re.search(r'info\([^)]*?\bversion\s*=\s*"([^"]*)"', source, re.S)
if literal and literal.group(1) != version:
    errors.append(f"src/api/openapi.rs info version {literal.group(1)!r} != {version}")

# apps/web is private and carries no version today; if one appears it must match.
for rel in ("apps/web/package.json",):
    pkg = json.loads((root / rel).read_text(encoding="utf-8"))
    if "version" in pkg and pkg["version"] != version:
        errors.append(f"{rel} version {pkg['version']!r} != {version}")

tag = os.environ["TAG"]
if tag and tag != f"v{version}":
    errors.append(f"tag {tag!r} != v{version}")
image_tag = os.environ["IMAGE_TAG"]
if image_tag and image_tag != version:
    errors.append(f"image tag {image_tag!r} != {version}")

if errors:
    for err in errors:
        print(f"release-check-version: {err}", file=sys.stderr)
    sys.exit(1)
print(version)
PY
