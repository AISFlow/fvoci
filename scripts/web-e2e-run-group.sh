#!/usr/bin/env bash
# Run one isolated web e2e group (fresh DB/role/search index/server/storage
# per invocation, in its own or in caller-shared test services).
set -euo pipefail

: "${ROOT:?ROOT is required}"
: "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is required}"

# The timer's full suite exceeds the ordinary login budget in one fresh app.
# Keep recovery/control login budgets and the restart fixture's fresh DB/server
# lifecycle in independent groups alongside the original cases.
# Explicit selection keeps the caller's arguments and runtime scope intact.
TIMER_SPEC_COUNT=0
TIMER_OTHER_SPEC=false
TIMER_EXPLICIT_SELECTION=false
for arg in "$@"; do
  case "$arg" in
    v050-task-timer.spec.ts|e2e/v050-task-timer.spec.ts)
      TIMER_SPEC_COUNT=$((TIMER_SPEC_COUNT + 1))
      ;;
    *.spec.ts) TIMER_OTHER_SPEC=true ;;
    --grep|--grep=*|--grep-invert|--grep-invert=*|-g|-g?*|--shard|--shard=*|--list|--)
      TIMER_EXPLICIT_SELECTION=true
      ;;
  esac
done
if ((TIMER_SPEC_COUNT == 1)) && [[ "$TIMER_OTHER_SPEC" == false && "$TIMER_EXPLICIT_SELECTION" == false && "${FVOCI_E2E_PENDING:-}" != 1 ]]; then
  TIMER_NEW_CONTROL_FILTER='ordinary research plan persists|task widget retires|a late task-widget R1|owner releases opaque legacy reservations|a late legacy release'
  TIMER_RECOVERY_FILTER='real browser offline start|a native committed pause|a planner A-B-A|an estimate A-B-A|a genuine new session retires|transient browser 429|one ordinary task restore'
  TIMER_RESTART_FILTER='native same-database restart'
  bash "$ROOT/scripts/web-e2e-run-group.sh" "$@" --grep-invert "$TIMER_NEW_CONTROL_FILTER|$TIMER_RECOVERY_FILTER|$TIMER_RESTART_FILTER"
  bash "$ROOT/scripts/web-e2e-run-group.sh" "$@" --grep "$TIMER_RECOVERY_FILTER"
  bash "$ROOT/scripts/web-e2e-run-group.sh" "$@" --grep "$TIMER_RESTART_FILTER"
  bash "$ROOT/scripts/web-e2e-run-group.sh" "$@" --grep "$TIMER_NEW_CONTROL_FILTER"
  exit 0
fi

# The label names the spec arguments; Playwright options (arguments that
# start with "-", such as --config=... after the spec; give option values in
# the same argument) are passed on but not named.
LABEL_SPECS=()
for arg in "$@"; do
  if [[ "$arg" != -* ]]; then
    LABEL_SPECS+=("$arg")
  fi
done

GROUP_LABEL="default-suite"
if [[ "${FVOCI_E2E_PENDING:-}" == "1" ]] && ((${#LABEL_SPECS[@]} < 1)); then
  GROUP_LABEL="collaboration-pending"
fi

if ((${#LABEL_SPECS[@]} >= 1)); then
  GROUP_LABEL="$(basename -- "${LABEL_SPECS[0]%.spec.ts}")"
  if ((${#LABEL_SPECS[@]} > 1)); then
    GROUP_LABEL="${GROUP_LABEL}+$(basename -- "${LABEL_SPECS[1]%.spec.ts}")"
  fi
fi

RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-web-e2e.XXXXXX")"
# Keep default timer screenshots/proofs in the group's retained output tree;
# an explicit caller namespace remains unchanged outside runtime cleanup.
export FVOCI_W5_EVIDENCE_DIR="${FVOCI_W5_EVIDENCE_DIR:-$RUN_DIR/playwright-output/w5-evidence}"
SERVER_LOG="$RUN_DIR/server.log"
PEPPER='{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}'
# Host netlink address/link events (only `ip monitor` writes this file, so its
# mtime is the last event; web-e2e-inner.sh relies on that) and the group's
# own markers, kept apart and merged by timestamp when retained.
NET_MONITOR_LOG="$RUN_DIR/net-monitor.log"
NET_MARKS_LOG="$RUN_DIR/net-marks.log"
NET_MONITOR_PID=""

# "[UTC time] # fvoci: msg", the `ip -tshort` prefix under TZ=UTC, so both
# files sort into one timeline. Bash's own clock is used because not every
# date(1) implementation zero-pads %6N.
net_mark() {
  # EPOCHREALTIME is bash 5.0+; without it the markers are skipped, never
  # an unbound-variable error under set -u (the cleanup trap calls this).
  local now="${EPOCHREALTIME:-}"
  [[ -n "$now" ]] || return 0
  TZ=UTC printf '[%(%Y-%m-%dT%H:%M:%S)T.%s] # fvoci: %s\n' \
    "${now%[.,]*}" "${now#*[.,]}" "$*" >>"$NET_MARKS_LOG"
}

# Chromium's network change notifier aborts in-flight requests with
# net::ERR_NETWORK_CHANGED on host address/link changes, and each group starts
# its containers just before the browser. Record the same netlink groups
# (address, link) from before the containers start until the group ends.
start_net_monitor() {
  : >"$NET_MONITOR_LOG"
  if ! command -v ip >/dev/null 2>&1; then
    net_mark "ip (iproute2) not found; no netlink event log"
    echo "warning: ip (iproute2) not found; no netlink event log for group ${GROUP_LABEL}" >&2
    return 0
  fi
  TZ=UTC ip -o -tshort monitor address link >>"$NET_MONITOR_LOG" 2>&1 </dev/null &
  NET_MONITOR_PID=$!
  net_mark "netlink monitor started (ip monitor address link, pid ${NET_MONITOR_PID})"
}

stop_net_monitor() {
  if [[ -n "$NET_MONITOR_PID" ]]; then
    kill "$NET_MONITOR_PID" 2>/dev/null || true
    wait "$NET_MONITOR_PID" 2>/dev/null || true
    NET_MONITOR_PID=""
  fi
}

redact_server_log() {
  sed -E \
    -e 's#postgres(ql)?://[^[:space:]]+#postgres://redacted#g' \
    -e 's#libsql://[^[:space:]]+#libsql://redacted#g' \
    -e 's#https://[^[:space:]]*\.turso\.io[^[:space:]]*#https://redacted#g' \
    -e 's#(DATABASE_URL|DATABASE_APP_URL|FVOCI_E2E_ADMIN_DATABASE_URL|TEST_DATABASE_URL|FVOCI_LIBSQL_URL|FVOCI_LIBSQL_AUTH_TOKEN|FVOCI_TEST_TURSO_[A-Z0-9_]*URL|FVOCI_TEST_TURSO_AUTH_TOKEN|MEILI[A-Z_]*KEY|PASSWORD[A-Z_]*|ENCRYPTION_KEYS)=[^[:space:]]+#\1=redacted#g' \
    "$1"
}

# Digest of one retained trace.zip (see tools/web-e2e/trace-summary.ts).
summarize_trace() {
  bun "$ROOT/tools/web-e2e/trace-summary.ts" "$1"
}

# Runs only after a failed group. Every step is attempted; a step that fails
# is named on stderr and makes this return 1, but never replaces a retained
# file with placeholder text. Self-contained: CI fixtures source it alone.
retain_failure_artifacts() {
  local retain_dir log dest trace err status errors=0
  local -a logs=() traces=() net_logs=()
  retention_error() {
    echo "web-e2e group ${GROUP_LABEL}: failure-artifact retention step $1 failed${2:+: $2}" >&2
    errors=$((errors + 1))
  }
  retain_dir="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-collab-e2e-fail.XXXXXX")" || {
    retention_error create-directory
    return 1
  }
  chmod 700 "$retain_dir" || retention_error chmod-directory
  # Named first, so a caller finds the directory even if this is cut short.
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    printf 'failure-artifacts=%s\nfailure-group=%s\n' "$retain_dir" "$GROUP_LABEL" >>"$GITHUB_OUTPUT" ||
      retention_error github-output
  fi
  if [[ -d "$RUN_DIR/playwright-output" ]] && [[ -n "$(ls -A "$RUN_DIR/playwright-output")" ]]; then
    cp -a "$RUN_DIR/playwright-output" "$retain_dir/playwright-output" || retention_error copy-playwright-output
  fi
  # The group's own server (ordinary runs); pending runs start servers per spec.
  if [[ -f "$SERVER_LOG" ]]; then
    redact_server_log "$SERVER_LOG" >"$retain_dir/server.log" || retention_error redact-server-log
  fi
  mkdir -p "$retain_dir/owned-server" || retention_error create-owned-server-directory
  mapfile -d '' -t logs < <(find "$RUN_DIR" -mindepth 2 -name server.log -type f -print0)
  wait "$!" || retention_error find-owned-server-logs
  for log in "${logs[@]}"; do
    dest="$retain_dir/owned-server/$(basename "$(dirname "$log")").log"
    redact_server_log "$log" >"$dest" || retention_error "redact-owned-server-log ($log)"
  done
  # Interface names, link flags and addresses only; redacted like the rest.
  for log in "$NET_MONITOR_LOG" "$NET_MARKS_LOG"; do
    if [[ -f "$log" ]]; then net_logs+=("$log"); fi
  done
  if ((${#net_logs[@]} > 0)); then
    {
      echo "# host netlink address/link events (ip -o -tshort monitor address link, UTC) and group markers"
      LC_ALL=C sort -s -k1,1 "${net_logs[@]}"
    } | redact_server_log /dev/stdin >"$retain_dir/net-events.log" || retention_error merge-net-events
  fi
  mapfile -d '' -t traces < <(find "$retain_dir" -name trace.zip -type f -print0)
  wait "$!" || retention_error find-traces
  for trace in "${traces[@]}"; do
    dest="$(dirname "$trace")/browser-summary.txt"
    err="$(dirname "$trace")/browser-summary.err"
    status=0
    summarize_trace "$trace" >"$dest" 2>"$err" || status=$?
    if ((status != 0)); then
      # No partial or placeholder summary; the summarizer's own error text is
      # shown redacted, and the trace.zip itself stays retained.
      rm -f "$dest"
      retention_error "trace-summary ($(basename "$(dirname "$trace")")/trace.zip, exit ${status})" \
        "$(redact_server_log "$err" | tail -n 5 | tr '\n' ' ')"
    fi
    rm -f "$err"
  done
  if ((errors > 0)); then
    echo "retained incomplete failure artifacts for group ${GROUP_LABEL} in $retain_dir (${errors} retention steps failed)" >&2
    return 1
  fi
  echo "retained failure artifacts for group ${GROUP_LABEL} in $retain_dir" >&2
}

# The group's verdict is its runtime's exit status. Artifact retention after a
# failure never changes it; a run directory that cannot be removed fails an
# otherwise passing group.
cleanup() {
  local status=$?
  # After an interrupt the reader of stderr may be gone: a write must not
  # end this cleanup (SIGPIPE) before the retention and the removal below.
  trap '' PIPE
  # Each step below is checked; a failed write to stderr must not end it.
  set +e
  stop_net_monitor
  net_mark "group exiting with status ${status}" 2>/dev/null || true
  if (( status != 0 )); then
    retain_failure_artifacts ||
      echo "web-e2e group ${GROUP_LABEL}: failure artifacts are incomplete; the group verdict stays exit ${status}" >&2
  fi
  if ! rm -rf "$RUN_DIR"; then
    echo "web-e2e group ${GROUP_LABEL}: could not remove its run directory $RUN_DIR" >&2
    if (( status == 0 )); then status=1; fi
  fi
  exit "$status"
}
trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

if [[ ! -d "$ROOT/apps/web/dist" ]]; then
  echo "missing apps/web/dist; build web assets before running groups" >&2
  exit 1
fi

cp -a "$ROOT/apps/web/dist" "$RUN_DIR/static"

echo "=== web e2e group: ${GROUP_LABEL} ===" >&2
# The test services belong to whoever started them. A scope that runs groups
# one after another may start PostgreSQL and Meilisearch once and set
# FVOCI_E2E_SHARED_SERVICES=1; each group still creates and retires its own
# database, app role and Meilisearch index (web-e2e-inner.sh). Otherwise this
# group starts and removes its own containers.
service_wrappers=()
if [[ "${FVOCI_E2E_SHARED_SERVICES:-}" == 1 ]]; then
  # The exports of start-test-postgres.sh and start-test-meili.sh.
  for name in TEST_DATABASE_URL FVOCI_TEST_PG_CONTAINER FVOCI_MEILI_URL FVOCI_MEILI_KEY MEILI_MASTER_KEY FVOCI_TEST_MEILI_CONTAINER; do
    if [[ -z "${!name:-}" ]]; then
      echo "FVOCI_E2E_SHARED_SERVICES=1 requires ${name} from the scope that started the services" >&2
      exit 1
    fi
  done
else
  service_wrappers=(bash "$ROOT/scripts/start-test-postgres.sh" bash "$ROOT/scripts/start-test-meili.sh")
fi
start_net_monitor
if ((${#service_wrappers[@]} > 0)); then
  net_mark "starting test containers (postgres, meilisearch)"
else
  net_mark "using shared test containers (postgres, meilisearch)"
fi
"${service_wrappers[@]}" \
  env RUN_DIR="$RUN_DIR" SERVER_LOG="$SERVER_LOG" PEPPER="$PEPPER" ROOT="$ROOT" \
    CARGO_TARGET_DIR="$CARGO_TARGET_DIR" FVOCI_STATIC_DIR="$RUN_DIR/static" \
    NET_MONITOR_LOG="$NET_MONITOR_LOG" NET_MARKS_LOG="$NET_MARKS_LOG" \
    NET_MONITOR_PID="$NET_MONITOR_PID" \
  bash "$ROOT/scripts/web-e2e-inner.sh" "$@"
