#!/usr/bin/env bash
# One perf run: fresh DB + app role, release server on 127.0.0.1:0, Playwright.
set -euo pipefail

: "${ROOT:?}" "${RUN_DIR:?}" "${RELEASE:?}" "${FVOCI_PERF_OUT:?}" "${FVOCI_PERF_DATASET:?}" "${TEST_DATABASE_URL:?}"
PG_CONTAINER="${FVOCI_TEST_PG_CONTAINER:?missing test postgres container}"
SERVER_LOG="$RUN_DIR/server.log"
SERVER_PID=""
cleanup() {
  if [[ -n "$SERVER_PID" ]] && kill -0 "$SERVER_PID" 2>/dev/null; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
}
trap cleanup EXIT

psql_admin() { docker exec -i "$PG_CONTAINER" psql -U postgres -v ON_ERROR_STOP=1 "$@"; }
redact_server_log() {
  sed -E -e 's#postgres(ql)?://[^[:space:]]+#postgres://redacted#g' \
    -e 's#libsql://[^[:space:]]+#libsql://redacted#g' \
    -e 's#https://[^[:space:]]*\.turso\.io[^[:space:]]*#https://redacted#g' \
    -e 's#(DATABASE_URL|DATABASE_APP_URL|FVOCI_E2E_ADMIN_DATABASE_URL|TEST_DATABASE_URL|FVOCI_LIBSQL_URL|FVOCI_LIBSQL_AUTH_TOKEN|FVOCI_TEST_TURSO_[A-Z0-9_]*URL|FVOCI_TEST_TURSO_AUTH_TOKEN|MEILI[A-Z_]*KEY|PASSWORD[A-Z_]*|ENCRYPTION_KEYS)=[^[:space:]]+#\1=redacted#g' \
    "$SERVER_LOG"
}
# The outer runner removes RUN_DIR, so preserve startup diagnostics on stderr.
startup_failure() {
  echo "$1" >&2
  redact_server_log >&2
  exit 1
}

DB_NAME="fvoci_perf_$(openssl rand -hex 8)"
ROLE_NAME="fvoci_app_${DB_NAME}"
ROLE_PASSWORD="$(openssl rand -hex 16)"
refuse_admin_url() {
  echo "TEST_DATABASE_URL $1" >&2
  exit 1
}
# 1-4 hex digit groups joined by ':' (possibly none); prints the group count.
ipv6_groups() {
  [[ -z "$1" ]] && { echo 0; return; }
  [[ "$1" =~ ^[0-9a-f]{1,4}(:[0-9a-f]{1,4})*$ ]] || return 1
  local colons="${1//[!:]/}"
  echo $((${#colons} + 1))
}
# An IPv6 address, optionally ending in dotted IPv4, without zone ID.
valid_ipv6() {
  local addr="$1" last octet total left right
  [[ "$addr" == *:* ]] || return 1
  last="${addr##*:}"
  if [[ "$last" == *.* ]]; then
    [[ "$last" =~ ^(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})$ ]] || return 1
    for octet in "${BASH_REMATCH[@]:1}"; do ((octet <= 255)) || return 1; done
    addr="${addr%"$last"}0:0"
  fi
  if [[ "$addr" == *::* ]]; then
    right="${addr#*::}"
    [[ "$right" != *::* ]] || return 1
    left="$(ipv6_groups "${addr%%::*}")" && right="$(ipv6_groups "$right")" || return 1
    ((left + right <= 7))
  else
    total="$(ipv6_groups "$addr")" || return 1
    ((total == 8))
  fi
}
# Admin URL: TEST_DATABASE_URL with only its path replaced. App URL: the same
# host and port with the app role. ROLE_NAME and ROLE_PASSWORD are generated
# above from [0-9a-z_] only, so they need no percent-encoding. A malformed
# admin URL is refused here, before anything is created.
printable_ascii_url() {
  local LC_ALL=C
  [[ "$1" =~ ^[a-zA-Z][a-zA-Z0-9+.-]*://[!-~]*$ ]]
}
printable_ascii_url "$TEST_DATABASE_URL" || refuse_admin_url "is not a printable ASCII URL"
admin_scheme="${TEST_DATABASE_URL%%://*}"
admin_rest="${TEST_DATABASE_URL#*://}"
admin_netloc="${admin_rest%%[/?#]*}"
admin_tail="${admin_rest#"$admin_netloc"}"
admin_path="${admin_tail%%[?#]*}"
admin_hostport="${admin_netloc##*@}"
[[ "${admin_netloc%"$admin_hostport"}" != *[][]* ]] || refuse_admin_url "has a bracket in its userinfo"
if [[ "$admin_hostport" == \[* ]]; then
  admin_host="${admin_hostport%%]*}]"
  [[ "$admin_host" != "$admin_hostport]" ]] || refuse_admin_url "has an unclosed IPv6 bracket"
  admin_host="${admin_host,,}"
  valid_ipv6 "${admin_host:1:-1}" || refuse_admin_url "has an invalid IPv6 literal"
  admin_port="${admin_hostport#*]}"
  [[ -z "$admin_port" || "$admin_port" == :* ]] || refuse_admin_url "has text after its IPv6 literal"
else
  admin_host="${admin_hostport%%:*}"
  [[ "$admin_host" != *[][]* ]] || refuse_admin_url "has a bracket in its host"
  admin_port="${admin_hostport#"$admin_host"}"
  admin_host="${admin_host,,}"
fi
admin_port="${admin_port#:}"
if [[ -n "$admin_port" ]]; then
  [[ "$admin_port" =~ ^[0-9]+$ ]] || refuse_admin_url "has an invalid port"
  admin_port="${admin_port#"${admin_port%%[!0]*}"}"
  ((${#admin_port} <= 5 && ${admin_port:-0} >= 1 && ${admin_port:-0} <= 65535)) \
    || refuse_admin_url "has a port outside 1-65535"
fi
export DATABASE_URL="$admin_scheme://$admin_netloc/$DB_NAME${admin_tail#"$admin_path"}"
export DATABASE_APP_URL="postgres://$ROLE_NAME:$ROLE_PASSWORD@${admin_host:-127.0.0.1}:${admin_port:-5432}/$DB_NAME"
psql_admin -d postgres -c "CREATE DATABASE \"$DB_NAME\"" >/dev/null
"$RELEASE/fvoci-migrate" >/dev/null
psql_admin -d "$DB_NAME" -c "CREATE ROLE \"$ROLE_NAME\" LOGIN PASSWORD '$ROLE_PASSWORD' NOSUPERUSER NOBYPASSRLS" >/dev/null
"$RELEASE/fvoci-migrate" --grant-app-role "$ROLE_NAME" >/dev/null
PG_VERSION="$(psql_admin -d postgres -tAc 'SHOW server_version' | tr -d '[:space:]')"

export PASSWORD_PEPPER_KEYS='{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}'
export PASSWORD_PEPPER_ACTIVE_KEY_ID=test
ENCRYPTION_KEYS="{\"perf\":\"$(openssl rand -hex 32)\"}"
export ENCRYPTION_KEYS ENCRYPTION_ACTIVE_KEY_ID=perf
export FVOCI_BIND="127.0.0.1:0" FVOCI_PUBLIC_ORIGIN="http://127.0.0.1:0"
export FVOCI_STATIC_DIR="$RUN_DIR/static" FVOCI_STORAGE_DIR="$RUN_DIR/storage"
mkdir -p "$FVOCI_STORAGE_DIR"
export FVOCI_E2E_ADMIN_DATABASE_URL="$DATABASE_URL"
unset DATABASE_URL FVOCI_MIGRATION_URL

"$RELEASE/fvoci-server" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!
BASE_URL=""
SERVER_READY=0
for _ in $(seq 1 120); do
  BASE_URL="$(grep -m1 'fvoci-server listening on ' "$SERVER_LOG" 2>/dev/null | sed 's/.*listening on //' | tr -d '\r' || true)"
  kill -0 "$SERVER_PID" 2>/dev/null || startup_failure "server exited during startup"
  if [[ -n "$BASE_URL" ]] && curl -fsS "$BASE_URL/api/v1/setup" >/dev/null 2>&1; then
    SERVER_READY=1
    break
  fi
  sleep 0.25
done
# Discovering the endpoint is separate from a successful setup request.
((SERVER_READY == 1)) || startup_failure "server did not become ready within 30s (GET /api/v1/setup never succeeded)"

TAG="$(printf '%s' "${FVOCI_PERF_TAG:-}" | tr -cd 'A-Za-z0-9-')"
GREP_ARGS=()
if [[ -n "${FVOCI_PERF_GREP:-}" ]]; then
  GREP_ARGS=(--grep "$FVOCI_PERF_GREP")
  [[ -n "$TAG" ]] || { echo "FVOCI_PERF_GREP requires FVOCI_PERF_TAG" >&2; exit 1; }
fi
# Prints $1 as a JSON string body: '"', '\' and every C0 control character
# escaped. Non-ASCII bytes are refused instead of guessing their encoding.
json_string_body() {
  local LC_ALL=C s="$1" out="" c code i
  for ((i = 0; i < ${#s}; i++)); do
    c="${s:i:1}"
    printf -v code '%d' "'$c"
    if ((code < 0 || code > 127)); then
      return 1
    elif [[ "$c" == '"' || "$c" == "\\" ]]; then
      out+="\\$c"
    elif ((code < 32)); then
      printf -v c '\\u%04x' "$code"
      out+="$c"
    else
      out+="$c"
    fi
  done
  printf '%s' "$out"
}
pg_version_json="$(json_string_body "$PG_VERSION")" || { echo "server_version is not ASCII" >&2; exit 1; }
printf '{\n "dataset_run_started": "%s",\n "postgres_server_version": "%s",\n "network": "loopback 127.0.0.1",\n "server": "release fvoci-server (source build)"\n}' \
  "$(date -u +%Y-%m-%dT%H:%M:%S.%6N+00:00)" "$pg_version_json" >"$FVOCI_PERF_OUT/run-$FVOCI_PERF_DATASET$TAG.json"

cd "$ROOT/apps/web"
set +e
PLAYWRIGHT_BASE_URL="$BASE_URL" FVOCI_PERF_RUN_DIR="$RUN_DIR" CARGO_TARGET_DIR="$RUN_DIR/fixture-target" \
  bun --bun x --no-install playwright test --config=e2e/perf/perf.config.ts "${GREP_ARGS[@]}"
status=$?
set -e
# Keep only the count of server warnings/errors; the log itself stays in the run dir
# unless FVOCI_PERF_KEEP_SERVER_LOG=1 asks for a redacted copy (diagnosis only).
grep -cE ' (WARN|ERROR) ' "$SERVER_LOG" >"$FVOCI_PERF_OUT/server-warn-error-count-$FVOCI_PERF_DATASET$TAG.txt" || true
if [[ "${FVOCI_PERF_KEEP_SERVER_LOG:-}" == "1" ]]; then
  redact_server_log >"$FVOCI_PERF_OUT/server-log-$FVOCI_PERF_DATASET$TAG.txt"
fi
exit "$status"
