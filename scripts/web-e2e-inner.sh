#!/usr/bin/env bash
set -euo pipefail

: "${ROOT:?ROOT is required}"
: "${SERVER_LOG:?SERVER_LOG is required}"
: "${PEPPER:?PEPPER is required}"
: "${RUN_DIR:?RUN_DIR is required}"

CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
SERVER_BIN="$CARGO_TARGET_DIR/debug/fvoci-server"
MIGRATE_BIN="$CARGO_TARGET_DIR/debug/fvoci-migrate"

SERVER_PID=""
SMTP_PID=""
cleanup_server() {
  if [[ -n "${SERVER_PID}" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  if [[ -n "${SMTP_PID}" ]] && kill -0 "$SMTP_PID" 2>/dev/null; then
    kill "$SMTP_PID" 2>/dev/null || true
    wait "$SMTP_PID" 2>/dev/null || true
  fi
}
trap cleanup_server EXIT

PG_CONTAINER="${FVOCI_TEST_PG_CONTAINER:?missing test postgres container}"
psql_admin() {
  docker exec -i "$PG_CONTAINER" psql -U postgres -v ON_ERROR_STOP=1 "$@"
}

DB_NAME="fvoci_e2e_$(openssl rand -hex 8)"
ROLE_NAME="fvoci_app_$(echo "$DB_NAME" | tr '-' '_')"
ROLE_PASSWORD="$(openssl rand -hex 16)"
psql_admin -d postgres -c "CREATE DATABASE \"$DB_NAME\"" >/dev/null

mapfile -t _db_urls < <(python3 - <<PY
import os, urllib.parse
admin = urllib.parse.urlparse(os.environ["TEST_DATABASE_URL"])
db_name = "${DB_NAME}"
role = "${ROLE_NAME}"
role_password = "${ROLE_PASSWORD}"
admin_db = admin._replace(path=f"/{db_name}")
print(urllib.parse.urlunparse(admin_db))
host = admin.hostname or "127.0.0.1"
port = admin.port or 5432
user = urllib.parse.quote(role, safe="")
password = urllib.parse.quote(role_password, safe="")
print(f"postgres://{user}:{password}@{host}:{port}/{db_name}")
PY
)
DATABASE_URL="${_db_urls[0]}"
DATABASE_APP_URL="${_db_urls[1]}"
export DATABASE_URL DATABASE_APP_URL
"$MIGRATE_BIN" >/dev/null
psql_admin -d "$DB_NAME" -c "CREATE ROLE \"$ROLE_NAME\" LOGIN PASSWORD '$ROLE_PASSWORD' NOSUPERUSER NOBYPASSRLS" >/dev/null
"$MIGRATE_BIN" --grant-app-role "$ROLE_NAME"

export PASSWORD_PEPPER_KEYS="$PEPPER"
export PASSWORD_PEPPER_ACTIVE_KEY_ID=test
export FVOCI_BIND="127.0.0.1:0"
export FVOCI_PUBLIC_ORIGIN="http://127.0.0.1:0"
export FVOCI_STATIC_DIR="${FVOCI_STATIC_DIR:?run-web-e2e.sh must provide isolated static assets}"
# A run-owned directory is stable across server restarts and removed by the
# outer runner only after its owned servers have stopped.
export FVOCI_STORAGE_DIR="$RUN_DIR/storage"
mkdir -p "$FVOCI_STORAGE_DIR"
export FVOCI_E2E_ADMIN_DATABASE_URL="$DATABASE_URL"
export FVOCI_E2E_SERVER_BIN="$SERVER_BIN"
export FVOCI_E2E_RESULT_DIR="$RUN_DIR"

unset DATABASE_URL FVOCI_MIGRATION_URL

SMTP_CAPTURE="$RUN_DIR/smtp.jsonl"
SMTP_PORT_FILE="$RUN_DIR/smtp.port"
: >"$SMTP_CAPTURE"
python3 "$ROOT/scripts/smtp-sink.py" --capture "$SMTP_CAPTURE" --port-file "$SMTP_PORT_FILE" &
SMTP_PID=$!
deadline=$((SECONDS + 10))
until [[ -s "$SMTP_PORT_FILE" ]]; do
  if (( SECONDS >= deadline )); then
    echo "smtp sink did not write port file" >&2
    exit 1
  fi
  if ! kill -0 "$SMTP_PID" 2>/dev/null; then
    echo "smtp sink exited before becoming ready" >&2
    exit 1
  fi
  sleep 0.05
done
export SMTP_HOST="127.0.0.1"
export SMTP_PORT="$(cat "$SMTP_PORT_FILE")"
export SMTP_FROM="noreply@example.com"
export FVOCI_E2E_SMTP_CAPTURE="$SMTP_CAPTURE"

if [[ "${FVOCI_E2E_PENDING:-}" == "1" ]]; then
  cd "$ROOT/apps/web"
  "$ROOT/apps/web/node_modules/.bin/playwright" test \
    --config=e2e-pending/collab-playwright.config.ts "$@"
  exit 0
fi

"$SERVER_BIN" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

BASE_URL=""
for _ in $(seq 1 120); do
  BASE_URL="$(grep -m1 'fvoci-server listening on ' "$SERVER_LOG" 2>/dev/null | sed 's/.*listening on //' | tr -d '\r' || true)"
  if [[ -n "$BASE_URL" ]] && curl -fsS "$BASE_URL/api/v1/setup" >/dev/null 2>&1; then
    break
  fi
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    cat "$SERVER_LOG" >&2
    exit 1
  fi
  sleep 0.25
done

if [[ -z "$BASE_URL" ]]; then
  echo "server did not become ready within 30s" >&2
  cat "$SERVER_LOG" >&2
  exit 1
fi

cd "$ROOT/apps/web"
export PLAYWRIGHT_BASE_URL="$BASE_URL"
"$ROOT/apps/web/node_modules/.bin/playwright" test "$@"
