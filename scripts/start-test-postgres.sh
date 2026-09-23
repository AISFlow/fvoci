#!/usr/bin/env bash
set -euo pipefail

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required to start a local test PostgreSQL container" >&2
  exit 1
fi

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ $# -gt 0 ]]; then
  CMD=("$@")
else
  CMD=("$ROOT/scripts/run-db-tests.sh")
fi

IMAGE="postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db"
CONTAINER="fvoci-rust-test-pg-$(uuidgen | tr '[:upper:]' '[:lower:]')"
ENV_FILE="$(mktemp "${TMPDIR:-/tmp}/fvoci-pg-env.XXXXXX")"
chmod 600 "$ENV_FILE"
PASSWORD="$(openssl rand -hex 24)"
printf 'POSTGRES_PASSWORD=%s\n' "$PASSWORD" >"$ENV_FILE"

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  rm -f "$ENV_FILE"
}
trap cleanup EXIT

cid="$(docker run -d --rm \
  --name "$CONTAINER" \
  --env-file "$ENV_FILE" \
  -p 127.0.0.1:0:5432 \
  "$IMAGE")"

deadline=$((SECONDS + 30))
until docker exec "$cid" pg_isready -U postgres >/dev/null 2>&1; do
  if (( SECONDS >= deadline )); then
    echo "postgres did not become ready within 30s" >&2
    exit 1
  fi
  sleep 1
done

port="$(docker port "$cid" 5432 | head -1 | awk -F: '{print $NF}')"
export TEST_DATABASE_URL="postgres://postgres:${PASSWORD}@127.0.0.1:${port}/postgres"

exec "${CMD[@]}"
