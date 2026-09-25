#!/usr/bin/env bash
set -euo pipefail

for dependency in docker openssl; do
  command -v "$dependency" >/dev/null 2>&1 || {
    echo "$dependency is required for local test Meilisearch" >&2
    exit 1
  }
done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ $# -gt 0 ]]; then
  CMD=("$@")
else
  echo "usage: $0 <command> [args...]" >&2
  exit 1
fi

# Official CE v1.53.2 digest from .github/workflows/rust.yml
IMAGE="getmeili/meilisearch:v1.53.2@sha256:c94e58ca09662dd6e65e8f1b0fd145767be3da7d5422a863a27b8d2b68e090c9"
RUN_ID="$(openssl rand -hex 16)"
CONTAINER="fvoci-rust-test-meili-${RUN_ID}"
MASTER_KEY="$(openssl rand -hex 16)"

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

cid="$(docker run -d --rm \
  --name "$CONTAINER" \
  --label "fvoci.test-run=${RUN_ID}" \
  -e "MEILI_MASTER_KEY=${MASTER_KEY}" \
  -e MEILI_NO_ANALYTICS=true \
  -e MEILI_ENV=production \
  -p 127.0.0.1:0:7700 \
  "$IMAGE")"

deadline=$((SECONDS + 30))
until docker exec "$cid" wget -q -O /dev/null http://127.0.0.1:7700/health >/dev/null 2>&1; do
  if (( SECONDS >= deadline )); then
    echo "meilisearch did not become ready within 30s" >&2
    docker logs "$cid" >&2 || true
    exit 1
  fi
  sleep 1
done

port="$(docker port "$cid" 7700 | head -1 | awk -F: '{print $NF}')"
export FVOCI_MEILI_URL="http://127.0.0.1:${port}"
export FVOCI_MEILI_KEY="$MASTER_KEY"
export MEILI_MASTER_KEY="$MASTER_KEY"
export FVOCI_TEST_MEILI_CONTAINER="$CONTAINER"

"${CMD[@]}"
