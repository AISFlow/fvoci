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

# Markers for the group's netlink event log (see web-e2e-run-group.sh).
net_mark() {
  [[ -n "${NET_MARKS_LOG:-}" ]] || return 0
  # EPOCHREALTIME is bash 5.0+; without it the markers are skipped, never
  # an unbound-variable error under set -u (the cleanup trap calls this).
  local now="${EPOCHREALTIME:-}"
  [[ -n "$now" ]] || return 0
  TZ=UTC printf '[%(%Y-%m-%dT%H:%M:%S)T.%s] # fvoci: %s\n' \
    "${now%[.,]*}" "${now#*[.,]}" "$*" >>"$NET_MARKS_LOG"
}

net_event_count() {
  local count=0
  if [[ -n "${NET_MONITOR_LOG:-}" && -f "$NET_MONITOR_LOG" ]]; then
    count="$(wc -l <"$NET_MONITOR_LOG")"
  fi
  echo "$((count))"
}

# Chromium aborts in-flight requests with net::ERR_NETWORK_CHANGED when its
# netlink address tracker sees a host address or link change. The group's
# containers add veth links just before this point, and IPv6 duplicate address
# detection moves addresses from tentative to preferred a second or two later,
# which can land inside the first page load. Before the browser starts, wait
# until no IPv6 address is tentative and no address/link event arrived for
# QUIET_S. This is a bounded precondition, not a test timeout: after LIMIT_S
# it warns and continues. It returns at once when the host is already quiet.
settle_network_before_browser() {
  if ! python3 - <<'PY'
import datetime, os, subprocess, sys, time

LIMIT_S = 10.0
QUIET_S = 1.0
POLL_S = 0.1
monitor_log = os.environ.get("NET_MONITOR_LOG", "")
marks_log = os.environ.get("NET_MARKS_LOG", "")
monitor_pid = os.environ.get("NET_MONITOR_PID", "")


def say(message):
    print(f"network settle: {message}", file=sys.stderr, flush=True)
    if marks_log:
        now = datetime.datetime.now(datetime.timezone.utc)
        with open(marks_log, "a", encoding="utf-8") as marks:
            marks.write(f"[{now:%Y-%m-%dT%H:%M:%S.%f}] # fvoci: network settle: {message}\n")


def monitor_running():
    if not (monitor_pid and monitor_log and os.path.isfile(monitor_log)):
        return False
    try:
        os.kill(int(monitor_pid), 0)
    except (ValueError, OSError):
        return False
    return True


def tentative_interfaces():
    # dadfailed addresses stay tentative forever; they never settle.
    out = subprocess.run(
        ["ip", "-6", "-o", "addr", "show", "tentative", "-dadfailed"],
        check=True, capture_output=True, text=True, timeout=5,
    ).stdout
    return sorted({line.split()[1].rstrip(":") for line in out.splitlines() if len(line.split()) > 1})


def event_count():
    with open(monitor_log, "rb") as log:
        return sum(1 for _ in log)


start = time.monotonic()
try:
    tentative_interfaces()
    check_tentative = True
except (OSError, subprocess.SubprocessError) as error:
    check_tentative = False
    say(f"cannot list tentative addresses ({type(error).__name__})")
watch_events = monitor_running()
if not watch_events:
    say("netlink monitor not running; not checking for recent events")
if not (check_tentative or watch_events):
    say("skipped")
    sys.exit(0)

while True:
    elapsed = time.monotonic() - start
    tentative = tentative_interfaces() if check_tentative else []
    quiet = time.time() - os.stat(monitor_log).st_mtime if watch_events else None
    events = f"; netlink events since the group started: {event_count()}" if watch_events else ""
    if not tentative and (quiet is None or quiet >= QUIET_S):
        say(f"settled after {elapsed:.2f} s{events}")
        break
    if elapsed >= LIMIT_S:
        detail = f"tentative: {', '.join(tentative) or 'none'}"
        if quiet is not None:
            detail += f"; last netlink event {quiet:.2f} s ago"
        say(f"warning: host network still changing after {LIMIT_S:.0f} s ({detail}{events}); continuing")
        break
    time.sleep(POLL_S)
PY
  then
    echo "warning: network settle check failed; continuing" >&2
  fi
}

# run_playwright <args...>: mark the browser's lifetime in the netlink log and
# report how many address/link events arrived while it ran.
run_playwright() {
  local before status=0
  settle_network_before_browser
  before="$(net_event_count)"
  net_mark "playwright start"
  (cd "$ROOT/apps/web" && bun --bun x --no-install playwright test "$@") || status=$?
  net_mark "playwright exited with status ${status}"
  if [[ -n "${NET_MONITOR_PID:-}" ]] && kill -0 "$NET_MONITOR_PID" 2>/dev/null; then
    echo "network: netlink address/link events while Playwright ran: $(($(net_event_count) - before))" >&2
  fi
  return "$status"
}

net_mark "containers ready; preparing database"

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
# Webhook secrets are sealed with this run-only keyring; the integrations flow
# delivers to a receiver the spec binds on 127.0.0.1:0.
ENCRYPTION_KEYS="{\"e2e\":\"$(openssl rand -hex 32)\"}"
export ENCRYPTION_KEYS
export ENCRYPTION_ACTIVE_KEY_ID=e2e
export FVOCI_WEBHOOK_ALLOW_TARGETS=127.0.0.1
export FVOCI_BIND="127.0.0.1:0"
# Request spans and response events (method, route template, status,
# latency; no URI or headers, see src/http/request_trace.rs) so a failed
# group's retained server.log shows which requests the browser made. Other
# crates stay at warn; the server adds fvoci_server=info itself.
export RUST_LOG="${RUST_LOG:-warn,tower_http=debug}"
export FVOCI_PUBLIC_ORIGIN="http://127.0.0.1:0"
export FVOCI_STATIC_DIR="${FVOCI_STATIC_DIR:?run-web-e2e.sh must provide isolated static assets}"
# A run-owned directory is stable across server restarts and removed by the
# outer runner only after its owned servers have stopped.
export FVOCI_STORAGE_DIR="$RUN_DIR/storage"
mkdir -p "$FVOCI_STORAGE_DIR"
export FVOCI_E2E_ADMIN_DATABASE_URL="$DATABASE_URL"
export FVOCI_E2E_SERVER_BIN="$SERVER_BIN"
export FVOCI_E2E_RESULT_DIR="$RUN_DIR"
# Both configs write per-run results here; web-e2e-run-group.sh retains this
# exact directory on failure, so a shared default test-results/ is never used.
PLAYWRIGHT_OUTPUT_DIR="$RUN_DIR/playwright-output"
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
  run_playwright \
    --config=e2e-pending/collab-playwright.config.ts \
    --output="$PLAYWRIGHT_OUTPUT_DIR" "$@"
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

net_mark "server ready"
cd "$ROOT/apps/web"
export PLAYWRIGHT_BASE_URL="$BASE_URL"
run_playwright --output="$PLAYWRIGHT_OUTPUT_DIR" "$@"
