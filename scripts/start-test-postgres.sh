#!/usr/bin/env bash
set -euo pipefail

for dependency in docker openssl; do
  command -v "$dependency" >/dev/null 2>&1 || {
    echo "$dependency is required for local test PostgreSQL" >&2
    exit 1
  }
done

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ $# -gt 0 ]]; then
  CMD=("$@")
else
  CMD=("$ROOT/scripts/run-db-tests.sh")
fi

IMAGE="postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db"
RUN_ID="$(openssl rand -hex 16)"
CONTAINER="fvoci-rust-test-pg-${RUN_ID}"
ENV_FILE="$(mktemp "${TMPDIR:-/tmp}/fvoci-pg-env.XXXXXX")"
chmod 600 "$ENV_FILE"
PASSWORD="$(openssl rand -hex 24)"
printf 'POSTGRES_PASSWORD=%s\n' "$PASSWORD" >"$ENV_FILE"

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  rm -f "$ENV_FILE"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

PG_MAX_CONNECTIONS="${FVOCI_TEST_PG_MAX_CONNECTIONS:-100}"

cid="$(docker run -d --rm \
  --name "$CONTAINER" \
  --label "fvoci.test-run=${RUN_ID}" \
  --env-file "$ENV_FILE" \
  -p 127.0.0.1:0:5432 \
  "$IMAGE" \
  postgres -c "max_connections=${PG_MAX_CONNECTIONS}")"

deadline=$((SECONDS + 30))
until docker exec "$cid" pg_isready -h 127.0.0.1 -U postgres >/dev/null 2>&1; do
  if (( SECONDS >= deadline )); then
    echo "postgres did not become ready within 30s" >&2
    exit 1
  fi
  sleep 1
done

port="$(docker port "$cid" 5432 | head -1 | awk -F: '{print $NF}')"
export TEST_DATABASE_URL="postgres://postgres:${PASSWORD}@127.0.0.1:${port}/postgres"
export FVOCI_TEST_PG_CONTAINER="$CONTAINER"

# Keep this shell alive so EXIT cleans up after both successful and failed commands.
"${CMD[@]}"
