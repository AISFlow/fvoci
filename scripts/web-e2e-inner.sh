#!/usr/bin/env bash
# One web e2e group's runtime inside its test services: the group's own
# database and app role, Meilisearch index, SMTP sink and server, then
# Playwright. Everything this script creates is retired by its EXIT trap, on
# success, failure, SIGINT and SIGTERM; the services themselves belong to the
# caller (web-e2e-run-group.sh or the scope that shares them).
#
# Exit status precedence: Playwright's own nonzero exit, else the first failing
# setup step's exit (named on stderr as "web-e2e step <name> failed"), else 1
# when the group's fixtures could not be retired, else 0.
set -Eeuo pipefail

STEP=prepare
trap 'echo "web-e2e step ${STEP} failed (exit $?)" >&2' ERR

: "${ROOT:?ROOT is required}"
: "${SERVER_LOG:?SERVER_LOG is required}"
: "${PEPPER:?PEPPER is required}"
: "${RUN_DIR:?RUN_DIR is required}"

CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
# Cargo profile of the server and migrate binaries: debug for the normal
# groups; scripts/keycloak-oidc-e2e.sh runs the release build.
E2E_PROFILE="${FVOCI_E2E_PROFILE:-debug}"
case "$E2E_PROFILE" in
  debug | release) ;;
  *)
    echo "FVOCI_E2E_PROFILE must be debug or release" >&2
    exit 1
    ;;
esac
SERVER_BIN="$CARGO_TARGET_DIR/$E2E_PROFILE/fvoci-server"
MIGRATE_BIN="$CARGO_TARGET_DIR/$E2E_PROFILE/fvoci-migrate"

command -v setsid >/dev/null 2>&1 || {
  echo "setsid (util-linux) is required to run the group's server and SMTP sink in their own process groups" >&2
  exit 1
}

# Set once the matching fixture may exist; cleanup retires only these.
SERVER_PID=""
SMTP_PID=""
PLAYWRIGHT_PID=""
DB_NAME=""
ROLE_NAME=""
MEILI_INDEX=""
MEILI_SERVER_KEY_FILE=""

# The leader is still running (a zombie is not: `wait` reaps it). Without a
# readable /proc entry, a pid bash has not yet reaped counts as running, so
# the bounded loop below never blocks in `wait` on a live leader.
leader_running() {
  local stat
  if ! stat="$(cat "/proc/$1/stat" 2>/dev/null)"; then
    kill -0 "$1" 2>/dev/null
    return
  fi
  stat="${stat##*) }"
  [[ "${stat%% *}" != Z ]]
}

# group_gone <leader pid> <tenths of a second>: reap the leader and wait,
# bounded, until no member of its process group is left.
group_gone() {
  local pid="$1" i
  for ((i = 0; i < $2; i++)); do
    leader_running "$pid" || wait "$pid" 2>/dev/null || true
    kill -0 -- "-$pid" 2>/dev/null || return 0
    sleep 0.1
  done
  return 1
}

# leader_gone <leader pid> <tenths of a second>: bounded wait for the leader
# alone to exit and be reaped.
leader_gone() {
  local pid="$1" i
  for ((i = 0; i < $2; i++)); do
    if ! leader_running "$pid"; then
      wait "$pid" 2>/dev/null || true
      return 0
    fi
    sleep 0.1
  done
  return 1
}

# stop_process_group <name> <leader pid>: each long-running child runs as the
# leader of its own process group (setsid without a fork keeps this shell as
# its parent), so its own children are signalled with it. SIGTERM, a bounded
# grace period, then SIGKILL; fails when any member survives.
stop_process_group() {
  local name="$1" pid="$2"
  kill -TERM -- "-$pid" 2>/dev/null || true
  group_gone "$pid" 100 && return 0
  echo "web-e2e cleanup: ${name} process group ${pid} still running 10 s after SIGTERM; sending SIGKILL" >&2
  kill -KILL -- "-$pid" 2>/dev/null || true
  group_gone "$pid" 50 && return 0
  echo "web-e2e cleanup: ${name} process group ${pid} survived SIGKILL" >&2
  return 1
}

# meili_request <method> <path>: the response body, then the HTTP status on
# its own last line. Master key and URL reach curl through its stdin config,
# never its argv (start-test-meili.sh generates a hex key, so no config
# quoting is needed).
meili_request() {
  printf 'url = "%s%s"\nheader = "Authorization: Bearer %s"\n' \
    "${FVOCI_MEILI_URL%/}" "$2" "$MEILI_MASTER_KEY" |
    curl -sS --max-time 30 -X "$1" -w '\n%{http_code}' -K -
}

# Index deletion is an asynchronous Meilisearch task: HTTP 202 only enqueues
# it. Wait for the task to end (a completion wait, as wait_meili_task in
# src/search/meili.rs, not a retry; at most 60 s: 300 polls 0.2 s apart and
# never past the deadline), require success, then require the index to be gone.
# A group whose setup stopped before --ensure-meili-key created the index gets
# a failed task with index_not_found, which the 404 then confirms.
meili_delete_index() {
  local response task="" status="" deadline=$((SECONDS + 60)) i
  response="$(meili_request DELETE "/indexes/$MEILI_INDEX")" || return 1
  if [[ "${response##*$'\n'}" == 202 && "$response" =~ \"taskUid\":[[:space:]]*([0-9]+) ]]; then
    task="${BASH_REMATCH[1]}"
  fi
  if [[ -z "$task" ]]; then
    echo "web-e2e cleanup: Meilisearch index delete answered HTTP ${response##*$'\n'} without a task" >&2
    return 1
  fi
  for ((i = 0; i < 300 && SECONDS < deadline; i++)); do
    response="$(meili_request GET "/tasks/$task")" || return 1
    if [[ "${response##*$'\n'}" != 200 ]]; then
      echo "web-e2e cleanup: Meilisearch task ${task} answered HTTP ${response##*$'\n'}" >&2
      return 1
    fi
    status=""
    if [[ "$response" =~ \"status\":[[:space:]]*\"([a-z]+)\" ]]; then
      status="${BASH_REMATCH[1]}"
    fi
    case "$status" in
      succeeded | failed | canceled) break ;;
      *) sleep 0.2 ;;
    esac
  done
  if [[ "$status" == failed && "$response" =~ \"code\":[[:space:]]*\"index_not_found\" ]]; then
    status=succeeded
  fi
  if [[ "$status" != succeeded ]]; then
    echo "web-e2e cleanup: Meilisearch index deletion task ${task} did not succeed (status ${status:-unknown})" >&2
    return 1
  fi
  response="$(meili_request GET "/indexes/$MEILI_INDEX")" || return 1
  if [[ "${response##*$'\n'}" != 404 ]]; then
    echo "web-e2e cleanup: Meilisearch index still answers HTTP ${response##*$'\n'} after its deletion task succeeded" >&2
    return 1
  fi
}

# Key deletion is synchronous (204 No Content).
meili_delete_key() {
  local response
  response="$(meili_request DELETE "/keys/$1")" || return 1
  [[ "${response##*$'\n'}" == 204 ]] || {
    echo "web-e2e cleanup: Meilisearch key delete answered HTTP ${response##*$'\n'}" >&2
    return 1
  }
}

cleanup() {
  local status=$? failed=()
  trap - ERR
  # A second interrupt must not cut the retirement short; it is bounded.
  trap '' HUP INT TERM
  set +e
  if [[ -n "$PLAYWRIGHT_PID" ]]; then
    # After a forwarded interrupt Playwright finishes its own shutdown and
    # reports first; anything a spec left in its group is stopped after that.
    leader_gone "$PLAYWRIGHT_PID" 300 ||
      echo "web-e2e cleanup: Playwright still running 30 s after the group ended" >&2
    stop_process_group playwright "$PLAYWRIGHT_PID" || failed+=(stop-playwright)
  fi
  if [[ -n "$SERVER_PID" ]]; then
    stop_process_group server "$SERVER_PID" || failed+=(stop-server)
  fi
  if [[ -n "$SMTP_PID" ]]; then
    stop_process_group smtp-sink "$SMTP_PID" || failed+=(stop-smtp-sink)
  fi
  if [[ -n "$DB_NAME" ]]; then
    # FORCE ends connections a spec-owned server may still hold.
    timeout 60 docker exec -i "$PG_CONTAINER" psql -U postgres -v ON_ERROR_STOP=1 -d postgres \
      -c "DROP DATABASE IF EXISTS \"$DB_NAME\" WITH (FORCE)" >/dev/null || failed+=(drop-database)
    timeout 60 docker exec -i "$PG_CONTAINER" psql -U postgres -v ON_ERROR_STOP=1 -d postgres \
      -c "DROP ROLE IF EXISTS \"$ROLE_NAME\"" >/dev/null || failed+=(drop-role)
  fi
  if [[ -n "$MEILI_INDEX" ]]; then
    meili_delete_index || failed+=(delete-meili-index)
    if [[ -s "$MEILI_SERVER_KEY_FILE" ]]; then
      meili_delete_key "$(tr -d '[:space:]' <"$MEILI_SERVER_KEY_FILE")" || failed+=(delete-meili-key)
    fi
  fi
  if ((${#failed[@]} > 0)); then
    echo "web-e2e cleanup failed: ${failed[*]}" >&2
    if ((status == 0)); then
      status=1
    else
      echo "web-e2e: the group verdict stays exit ${status} (step ${STEP})" >&2
    fi
  fi
  exit "$status"
}
# Playwright runs in its own process group, so a terminal interrupt or a
# runner's TERM no longer reaches it directly: pass the signal on, then exit
# through cleanup, which waits for its shutdown.
forward_signal() {
  if [[ -n "$PLAYWRIGHT_PID" ]]; then
    kill "-$1" -- "-$PLAYWRIGHT_PID" 2>/dev/null || true
  fi
  exit "$2"
}
trap cleanup EXIT
trap 'forward_signal HUP 129' HUP
trap 'forward_signal INT 130' INT
trap 'forward_signal TERM 143' TERM

startup_failure() {
  echo "$1" >&2
  sed -E -e 's#postgres(ql)?://[^[:space:]]+#postgres://redacted#g' \
    -e 's#libsql://[^[:space:]]+#libsql://redacted#g' \
    -e 's#https://[^[:space:]]*\.turso\.io[^[:space:]]*#https://redacted#g' \
    -e 's#(DATABASE_URL|DATABASE_APP_URL|FVOCI_E2E_ADMIN_DATABASE_URL|TEST_DATABASE_URL|FVOCI_LIBSQL_URL|FVOCI_LIBSQL_AUTH_TOKEN|FVOCI_TEST_TURSO_[A-Z0-9_]*URL|FVOCI_TEST_TURSO_AUTH_TOKEN|MEILI[A-Z_]*KEY|PASSWORD[A-Z_]*|ENCRYPTION_KEYS)=[^[:space:]]+#\1=redacted#g' \
    "$SERVER_LOG" >&2
  exit 1
}

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
# it warns and continues (the warning is repeated next to a Playwright failure),
# and returns at once when the host is already quiet. A check that cannot run
# fails the group before the browser starts.
SETTLE_WARNING=""
settle_network_before_browser() {
  local settle_log="$RUN_DIR/network-settle.log" status=0
  python3 - 2>"$settle_log" <<'PY' || status=$?
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
  cat "$settle_log" >&2
  SETTLE_WARNING="$(grep -m1 '^network settle: warning: ' "$settle_log" || true)"
  return "$status"
}

# run_playwright <args...>: mark the browser's lifetime in the netlink log and
# report how many address/link events arrived while it ran.
run_playwright() {
  local before status=0
  STEP=network-settle
  settle_network_before_browser
  STEP=playwright
  before="$(net_event_count)"
  net_mark "playwright start"
  # Its own process group too: servers a spec starts without detaching stay
  # in it and are reaped with it. Waiting in the background keeps this shell
  # able to forward an interrupt (see forward_signal).
  setsid bash -c 'cd "$1" && shift && exec bun --bun x --no-install playwright test "$@"' \
    playwright "$ROOT/apps/web" "$@" &
  PLAYWRIGHT_PID=$!
  wait "$PLAYWRIGHT_PID" || status=$?
  net_mark "playwright exited with status ${status}"
  if [[ -n "${NET_MONITOR_PID:-}" ]] && kill -0 "$NET_MONITOR_PID" 2>/dev/null; then
    echo "network: netlink address/link events while Playwright ran: $(($(net_event_count) - before))" >&2
  fi
  if ((status != 0)) && [[ -n "$SETTLE_WARNING" ]]; then
    echo "web-e2e: Playwright started before the host network settled (${SETTLE_WARNING#network settle: })" >&2
  fi
  return "$status"
}

net_mark "containers ready; preparing database"

PG_CONTAINER="${FVOCI_TEST_PG_CONTAINER:?missing test postgres container}"
psql_admin() {
  docker exec -i "$PG_CONTAINER" psql -U postgres -v ON_ERROR_STOP=1 "$@"
}

STEP=create-database
DB_SUFFIX="$(openssl rand -hex 8)"
ROLE_PASSWORD="$(openssl rand -hex 16)"
# Named before the attempt: cleanup drops them IF EXISTS.
DB_NAME="fvoci_e2e_${DB_SUFFIX}"
ROLE_NAME="fvoci_app_${DB_NAME}"
psql_admin -d postgres -c "CREATE DATABASE \"$DB_NAME\"" >/dev/null

STEP=database-url
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
wait "$!"
((${#_db_urls[@]} == 2))
DATABASE_URL="${_db_urls[0]}"
DATABASE_APP_URL="${_db_urls[1]}"
export DATABASE_URL DATABASE_APP_URL
STEP=migrate
"$MIGRATE_BIN" >/dev/null
STEP=create-app-role
psql_admin -d "$DB_NAME" -c "CREATE ROLE \"$ROLE_NAME\" LOGIN PASSWORD '$ROLE_PASSWORD' NOSUPERUSER NOBYPASSRLS" >/dev/null
STEP=grant-app-role
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
export FVOCI_EXTRACT_POLL_SECS=2
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
STEP=smtp-sink
setsid bun "$ROOT/tools/web-e2e/smtp-sink.ts" --capture "$SMTP_CAPTURE" --port-file "$SMTP_PORT_FILE" &
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

# The normal server runs with runtime settings only: the setup/admin/test
# database URLs and the Meilisearch master key stay with the setup and test
# drivers. Search uses the scoped key the existing preparation command writes.
server_env=(env -u FVOCI_E2E_ADMIN_DATABASE_URL -u TEST_DATABASE_URL -u FVOCI_TEST_DATABASE_URL
  -u MEILI_MASTER_KEY -u FVOCI_MEILI_MASTER_KEY -u FVOCI_MEILI_KEY)
if [[ -n "${FVOCI_MEILI_URL:-}" && -n "${MEILI_MASTER_KEY:-}" ]]; then
  # The group's own index (and a key scoped to it), so groups sharing one
  # Meilisearch never see each other's documents. Specs read the same name.
  STEP=meili-key
  MEILI_SERVER_KEY_FILE="$RUN_DIR/meili-search.key"
  MEILI_INDEX="fvoci_e2e_${DB_SUFFIX}"
  export FVOCI_MEILI_INDEX="$MEILI_INDEX"
  "$MIGRATE_BIN" --ensure-meili-key "$MEILI_SERVER_KEY_FILE" >/dev/null
  server_env+=("FVOCI_MEILI_KEY_FILE=$MEILI_SERVER_KEY_FILE")
fi
# Launch proof, names only: the variables present under the exact server env
# prefix (values are never written). Specs may read this file.
STEP=server-start
SERVER_ENV_NAMES="$RUN_DIR/server-env-names.txt"
( umask 077 && "${server_env[@]}" bash -c 'compgen -e' | LC_ALL=C sort >"$SERVER_ENV_NAMES" )
export FVOCI_E2E_SERVER_ENV_NAMES="$SERVER_ENV_NAMES"
setsid "${server_env[@]}" "$SERVER_BIN" >"$SERVER_LOG" 2>&1 &
SERVER_PID=$!

BASE_URL=""
SERVER_READY=0
for _ in $(seq 1 120); do
  BASE_URL="$(grep -m1 'fvoci-server listening on ' "$SERVER_LOG" 2>/dev/null | sed 's/.*listening on //' | tr -d '\r' || true)"
  if ! kill -0 "$SERVER_PID" 2>/dev/null; then
    startup_failure "server exited during startup"
  fi
  if [[ -n "$BASE_URL" ]] && curl -fsS "$BASE_URL/api/v1/setup" >/dev/null 2>&1; then
    SERVER_READY=1
    break
  fi
  sleep 0.25
done

# Discovering the endpoint is separate from a successful setup request.
if (( SERVER_READY == 0 )); then
  startup_failure "server did not become ready within 30s (GET /api/v1/setup never succeeded)"
fi

net_mark "server ready"
cd "$ROOT/apps/web"
export PLAYWRIGHT_BASE_URL="$BASE_URL"
run_playwright --output="$PLAYWRIGHT_OUTPUT_DIR" "$@"
