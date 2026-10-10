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

TAG="$TAG" IMAGE_TAG="$IMAGE_TAG" bun "$ROOT/tools/release/version.ts" "$ROOT"
