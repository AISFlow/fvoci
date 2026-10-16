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

# Source-supported PostgreSQL majors; keep pins in sync with the postgres job
# matrix in .github/workflows/rust.yml.
PG_MAJOR="${FVOCI_TEST_PG_MAJOR-18}"
case "$PG_MAJOR" in
  16) IMAGE="postgres:16.15@sha256:ca0bd484cb98bf4b24eb1010e73fb3fcbd6714d240fbc1a10eea5b7dbecb641d" ;;
  17) IMAGE="postgres:17.11@sha256:2d2b8998d31037bf721cfdf764d76ba74171b4fab3431b7f72c27c56ddbdf9e3" ;;
  18) IMAGE="postgres:18.6@sha256:74935e72241653ca55e0414067e6d8763aceb8a810eb51b452253ec3dcfc4336" ;;
  *)
    echo "FVOCI_TEST_PG_MAJOR must be 16, 17 or 18 (got '$PG_MAJOR')" >&2
    exit 2
    ;;
esac
RUN_ID="$(openssl rand -hex 16)"
CONTAINER="fvoci-rust-test-pg-${RUN_ID}"
ENV_FILE="$(mktemp "${TMPDIR:-/tmp}/fvoci-pg-env.XXXXXX")"
chmod 600 "$ENV_FILE"
PASSWORD="$(openssl rand -hex 24)"
printf 'POSTGRES_PASSWORD=%s\n' "$PASSWORD" >"$ENV_FILE"

cleanup() {
  # -v: the image declares a VOLUME; without it every run leaves an anonymous
  # volume behind (the --rm auto-removal does not run after an explicit rm).
  docker rm -f -v "$CONTAINER" >/dev/null 2>&1 || true
  rm -f "$ENV_FILE"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

PG_MAX_CONNECTIONS="${FVOCI_TEST_PG_MAX_CONNECTIONS:-150}"

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

server_version_num="$(docker exec "$cid" psql -U postgres -tAc 'SHOW server_version_num')"
if [[ ! "$server_version_num" =~ ^${PG_MAJOR}[0-9]{4}$ ]]; then
  echo "expected PostgreSQL ${PG_MAJOR}, got server_version_num ${server_version_num}" >&2
  exit 1
fi

port="$(docker port "$cid" 5432 | head -1 | awk -F: '{print $NF}')"
export TEST_DATABASE_URL="postgres://postgres:${PASSWORD}@127.0.0.1:${port}/postgres"
export FVOCI_TEST_PG_CONTAINER="$CONTAINER"

# Keep this shell alive so EXIT cleans up after both successful and failed commands.
"${CMD[@]}"
