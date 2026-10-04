#!/usr/bin/env bash
# Run one isolated web e2e group (fresh DB/app/server/storage per invocation).
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
    -e 's#(DATABASE_URL|DATABASE_APP_URL|FVOCI_E2E_ADMIN_DATABASE_URL|TEST_DATABASE_URL)=[^[:space:]]+#\1=redacted#g' \
    "$1"
}

# Digest of one retained trace.zip (see scripts/web-e2e-trace-summary.py).
summarize_trace() {
  python3 "$ROOT/scripts/web-e2e-trace-summary.py" "$1"
}

retain_failure_artifacts() {
  local retain_dir log dest trace
  retain_dir="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-collab-e2e-fail.XXXXXX")" || return 1
  chmod 700 "$retain_dir"
  # Named first, so a caller finds the directory even if this is cut short.
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    printf 'failure-artifacts=%s\n' "$retain_dir" >>"$GITHUB_OUTPUT"
    printf 'failure-group=%s\n' "$GROUP_LABEL" >>"$GITHUB_OUTPUT"
  fi
  if [[ -d "$RUN_DIR/playwright-output" ]] && [[ -n "$(ls -A "$RUN_DIR/playwright-output" 2>/dev/null || true)" ]]; then
    cp -a "$RUN_DIR/playwright-output" "$retain_dir/playwright-output"
  fi
  # The group's own server (ordinary runs); pending runs start servers per spec.
  if [[ -f "$SERVER_LOG" ]]; then
    redact_server_log "$SERVER_LOG" >"$retain_dir/server.log"
  fi
  mkdir -p "$retain_dir/owned-server"
  while IFS= read -r -d '' log; do
    dest="$retain_dir/owned-server/$(basename "$(dirname "$log")").log"
    redact_server_log "$log" >"$dest"
  done < <(find "$RUN_DIR" -mindepth 2 -name server.log -type f -print0 2>/dev/null || true)
  # Interface names, link flags and addresses only; redacted like the rest.
  local -a net_logs=()
  for log in "$NET_MONITOR_LOG" "$NET_MARKS_LOG"; do
    [[ -f "$log" ]] && net_logs+=("$log")
  done
  if ((${#net_logs[@]} > 0)); then
    redact_server_log <(
      echo "# host netlink address/link events (ip -o -tshort monitor address link, UTC) and group markers"
      LC_ALL=C sort -s -k1,1 "${net_logs[@]}" || true
    ) >"$retain_dir/net-events.log"
  fi
  while IFS= read -r -d '' trace; do
    dest="$(dirname "$trace")/browser-summary.txt"
    # Python's own error text is not redacted, so it never reaches the file.
    if ! summarize_trace "$trace" >"$dest" 2>/dev/null; then
      echo "trace summary failed; reproduce the group locally to inspect its trace.zip" >"$dest"
      echo "could not summarize $(basename "$(dirname "$trace")")/trace.zip" >&2
    fi
  done < <(find "$retain_dir" -name trace.zip -type f -print0 2>/dev/null || true)
  echo "retained failure artifacts for group ${GROUP_LABEL} in $retain_dir" >&2
}

cleanup() {
  local status=$?
  # After an interrupt the reader of stderr may be gone: a write must not
  # end this cleanup (SIGPIPE) before the retention and the removal below.
  trap '' PIPE
  stop_net_monitor
  net_mark "group exiting with status ${status}" 2>/dev/null || true
  if (( status != 0 )); then
    retain_failure_artifacts || true
  fi
  rm -rf "$RUN_DIR"
}
trap cleanup EXIT

if [[ ! -d "$ROOT/apps/web/dist" ]]; then
  echo "missing apps/web/dist; build web assets before running groups" >&2
  exit 1
fi

cp -a "$ROOT/apps/web/dist" "$RUN_DIR/static"

echo "=== web e2e group: ${GROUP_LABEL} ===" >&2
start_net_monitor
net_mark "starting test containers (postgres, meilisearch)"
bash "$ROOT/scripts/start-test-postgres.sh" \
  bash "$ROOT/scripts/start-test-meili.sh" \
  env RUN_DIR="$RUN_DIR" SERVER_LOG="$SERVER_LOG" PEPPER="$PEPPER" ROOT="$ROOT" \
    CARGO_TARGET_DIR="$CARGO_TARGET_DIR" FVOCI_STATIC_DIR="$RUN_DIR/static" \
    NET_MONITOR_LOG="$NET_MONITOR_LOG" NET_MARKS_LOG="$NET_MARKS_LOG" \
    NET_MONITOR_PID="$NET_MONITOR_PID" \
  bash "$ROOT/scripts/web-e2e-inner.sh" "$@"
