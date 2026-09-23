#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ADMIN_URL="${TEST_DATABASE_URL:-}"
if [[ -z "$ADMIN_URL" && -f /tmp/fvoci-rust-b01d-jpu9ngdq/admin-url ]]; then
  ADMIN_URL="$(cat /tmp/fvoci-rust-b01d-jpu9ngdq/admin-url)"
fi
if [[ -z "$ADMIN_URL" ]]; then
  echo "TEST_DATABASE_URL or coordinator admin-url file is required" >&2
  exit 1
fi

DB_NAME="fvoci_test_$(date +%s)_$RANDOM"
ROLE_NAME="fvoci_app_${DB_NAME//-/_}"
ROLE_PASSWORD="$(openssl rand -hex 24)"
SERVER_URL="${ADMIN_URL%/*}"

psql "$SERVER_URL" -v ON_ERROR_STOP=1 -c "CREATE DATABASE \"$DB_NAME\""
MIGRATION_URL="$SERVER_URL/$DB_NAME"
export DATABASE_URL="$MIGRATION_URL"
export DATABASE_APP_URL=""

cargo run --quiet --bin fvoci-migrate 2>/dev/null || {
  export CARGO_HOME="${CARGO_HOME:-/home/kinesis/orca/toolchains/fvoci-rust/cargo}"
  export RUSTUP_HOME="${RUSTUP_HOME:-/home/kinesis/orca/toolchains/fvoci-rust/rustup}"
  export PATH="/home/kinesis/orca/toolchains/fvoci-rust/cargo/bin:$PATH"
  (cd "$ROOT" && cargo run --quiet --bin fvoci-migrate)
}

psql "$MIGRATION_URL" -v ON_ERROR_STOP=1 \
  -c "CREATE ROLE \"$ROLE_NAME\" LOGIN PASSWORD '$ROLE_PASSWORD' NOSUPERUSER NOBYPASSRLS"
psql "$MIGRATION_URL" -v ON_ERROR_STOP=1 -v app_role="$ROLE_NAME" -f "$ROOT/scripts/grant-app-role.sql"

PARSED="$(python3 -c "import os,urllib.parse; u=urllib.parse.urlparse('$MIGRATION_URL'); print(urllib.parse.urlunparse(u._replace(username='$ROLE_NAME', password='$ROLE_PASSWORD')))")"
export TEST_DATABASE_URL="$MIGRATION_URL"
export TEST_APP_DATABASE_URL="$PARSED"
export TEST_DB_NAME="$DB_NAME"
export TEST_APP_ROLE="$ROLE_NAME"

echo "TEST_DATABASE_URL set (admin, not printed)"
echo "TEST_APP_DATABASE_URL set (app role, not printed)"
