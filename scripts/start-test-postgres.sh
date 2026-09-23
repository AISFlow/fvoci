#!/usr/bin/env bash
set -euo pipefail

if ! command -v docker >/dev/null 2>&1; then
  echo "docker is required to start a local test PostgreSQL container" >&2
  exit 1
fi

IMAGE="postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db"
CONTAINER="fvoci-rust-test-pg-$(uuidgen | tr '[:upper:]' '[:lower:]')"
SECRET_FILE="$(mktemp "${TMPDIR:-/tmp}/fvoci-pg-secret.XXXXXX")"
chmod 600 "$SECRET_FILE"
PASSWORD="$(openssl rand -hex 24)"
printf '%s' "$PASSWORD" >"$SECRET_FILE"

cleanup() {
  docker rm -f "$CONTAINER" >/dev/null 2>&1 || true
  rm -f "$SECRET_FILE"
}
trap cleanup EXIT

cid="$(docker run -d --rm \
  --name "$CONTAINER" \
  -e "POSTGRES_PASSWORD=${PASSWORD}" \
  -p 127.0.0.1:0:5432 \
  "$IMAGE")"

port="$(docker port "$cid" 5432 | head -1 | awk -F: '{print $NF}')"
export TEST_DATABASE_URL="postgres://postgres:${PASSWORD}@127.0.0.1:${port}/postgres"

cat <<EOF
Started ephemeral PostgreSQL container: ${CONTAINER}
TEST_DATABASE_URL is set for this shell (password not echoed).
Secret file: ${SECRET_FILE} (mode 600)

Cleanup:
  docker rm -f ${CONTAINER}
  rm -f ${SECRET_FILE}
EOF
