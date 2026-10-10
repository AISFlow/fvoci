#!/usr/bin/env bash
# Test-only: run the real web-e2e-run-group.sh / web-e2e-inner.sh and the real
# Playwright CLI with the real configs against controlled page-less specs, and
# check that a failure's error-context.md lands in the retained
# playwright-output directory that CI uploads, for ordinary and pending runs,
# with the group's redacted netlink event log, and that the pre-browser network
# settle wait returns, waits for tentative addresses, and stays bounded.
# PostgreSQL, Meilisearch, migrations, the server and `ip` are stubbed; the real
# Bun SMTP sink runs on loopback. No browser is launched because the fixture
# specs never request `page`.
# Both inner wrappers must reject listening-only/early-exit servers and launch
# Playwright exactly once when setup becomes healthy on the final startup poll.
# The group retires what it created (database, app role, Meilisearch index and
# key, server with its child, SMTP sink) on pass, failure, SIGINT and a SIGTERM
# that also ends the output reader; a Meilisearch index counts as deleted only
# once its deletion task succeeded and the index is gone; failing steps are
# named; retention errors never change the verdict; and services a scope
# shares are started once for its groups.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
# The web workspace's install (bun ci at the repository root).
WORKSPACE_MODULES="$ROOT/node_modules"
REAL_BUN="$(command -v bun)"
REAL_SLEEP="$(command -v sleep)"
REAL_SEQ="$(command -v seq)"
if ! (cd "$ROOT/apps/web" && bun --bun x --no-install playwright --version) >/dev/null 2>&1; then
  echo "missing the locked Playwright install; run bun ci" >&2
  exit 1
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-web-e2e-failure-output.XXXXXX")"
cleanup() {
  local environ pid
  # After a failed case, stop everything a group left running (wrappers,
  # Playwright and its workers, the server, its child, the SMTP sink, spec
  # children): exactly the processes started with this fixture's own state
  # path in their environment, so no other process is ever signalled.
  for environ in /proc/[0-9]*/environ; do
    [[ -n "${NET_STATE:-}" ]] || break
    pid="${environ#/proc/}"
    pid="${pid%/environ}"
    ((pid != $$)) || continue
    if grep -qzxF "FVOCI_FIXTURE_NET_STATE=$NET_STATE" "$environ" 2>/dev/null; then
      kill -KILL "$pid" 2>/dev/null || true
    fi
  done
  rm -rf "$WORK"
}
trap cleanup EXIT

FIXTURE_ROOT="$WORK/root"
FAKE_BIN="$WORK/bin"
FIXTURE_PATH="$FAKE_BIN:$PATH"
RUN_TMP="$WORK/tmp"
NET_STATE="$WORK/net"
mkdir -p "$FIXTURE_ROOT/scripts/perf" "$FIXTURE_ROOT/tools/perf" "$FIXTURE_ROOT/tools/web-e2e" "$FIXTURE_ROOT/apps/web/e2e" \
  "$FIXTURE_ROOT/apps/web/e2e-pending" "$FIXTURE_ROOT/apps/web/dist" \
  "$FIXTURE_ROOT/target/debug" "$FIXTURE_ROOT/tools/web-e2e" "$FAKE_BIN" "$RUN_TMP"
cp "$ROOT/scripts/web-e2e-run-group.sh" "$ROOT/scripts/web-e2e-inner.sh" "$FIXTURE_ROOT/scripts/"
cp "$ROOT/tools/web-e2e/trace-summary.ts" "$ROOT/tools/web-e2e/compat.ts" "$FIXTURE_ROOT/tools/web-e2e/"
cp "$ROOT/scripts/perf/perf-inner.sh" "$FIXTURE_ROOT/scripts/perf/"
cp "$ROOT/tools/perf/perf-inner.ts" "$FIXTURE_ROOT/tools/perf/"
cp "$ROOT/tools/web-e2e/smtp-sink.ts" "$ROOT/tools/web-e2e/network-settle.ts" \
  "$ROOT/tools/web-e2e/database-urls.ts" "$FIXTURE_ROOT/tools/web-e2e/"
cp "$ROOT/apps/web/playwright.config.ts" "$FIXTURE_ROOT/apps/web/"
cp "$ROOT/apps/web/e2e-pending/collab-playwright.config.ts" "$FIXTURE_ROOT/apps/web/e2e-pending/"
ln -s "$WORKSPACE_MODULES" "$FIXTURE_ROOT/node_modules"
echo "fixture" >"$FIXTURE_ROOT/apps/web/dist/index.html"

for dir in e2e e2e-pending; do
  cat >"$FIXTURE_ROOT/apps/web/$dir/controlled-failure.spec.ts" <<'SPEC'
import { spawn } from "node:child_process";
import { writeFileSync } from "node:fs";
import { expect, test } from "@playwright/test";

test("controlled pass", async () => {
  if (process.env.FVOCI_FIXTURE_OUTCOME === "hang") {
    // Interrupt case: a spec-started child that outlives SIGINT (as a server
    // started during teardown does), then tell the fixture the browser phase
    // is running and wait.
    const sleep = process.env.FVOCI_FIXTURE_REAL_SLEEP ?? "sleep";
    const ignored = process.env.FVOCI_FIXTURE_TERM_RESIST === "1" ? "INT TERM" : "INT";
    const child = spawn("bash", ["-c", `trap "" ${ignored}; exec "$0" 600`, sleep], { stdio: "ignore" });
    writeFileSync(`${process.env.FVOCI_FIXTURE_NET_STATE}.spec-child-pid`, String(child.pid));
    writeFileSync(`${process.env.FVOCI_FIXTURE_NET_STATE}.hanging`, "");
    await new Promise(() => undefined);
  }
  expect(1).toBe(1);
});

test("controlled failure", async () => {
  expect(process.env.FVOCI_FIXTURE_OUTCOME).toBe("pass");
});
SPEC
done

# Service stubs record each start, so a shared scope can prove one start.
cat >"$FIXTURE_ROOT/scripts/start-test-postgres.sh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
echo "postgres" >>"$FVOCI_FIXTURE_NET_STATE.service-starts"
export TEST_DATABASE_URL="postgres://postgres:fixture-secret@127.0.0.1:5432/postgres"
export FVOCI_TEST_PG_CONTAINER="fvoci-fixture-pg"
"$@"
STUB
# Meilisearch is reachable only when FVOCI_FIXTURE_MEILI=1 (fake curl below).
cat >"$FIXTURE_ROOT/scripts/start-test-meili.sh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
echo "meili" >>"$FVOCI_FIXTURE_NET_STATE.service-starts"
if [[ "${FVOCI_FIXTURE_MEILI:-0}" == 1 ]]; then
  export FVOCI_MEILI_URL="http://127.0.0.1:7"
  export MEILI_MASTER_KEY="fixture-meili-master-secret"
  export FVOCI_MEILI_KEY="$MEILI_MASTER_KEY"
  export FVOCI_TEST_MEILI_CONTAINER="fvoci-fixture-meili"
fi
"$@"
STUB
# FVOCI_FIXTURE_MIGRATE=fail fails the schema migration; --ensure-meili-key
# records the index it was given and writes a fake scoped key.
cat >"$FIXTURE_ROOT/target/debug/fvoci-migrate" <<'STUB'
#!/usr/bin/env bash
if [[ "$#" == 0 && "${FVOCI_FIXTURE_MIGRATE:-ok}" == fail ]]; then
  echo "fixture migration failure" >&2
  exit 9
fi
if [[ "${1:-}" == "--ensure-meili-key" ]]; then
  echo "${FVOCI_MEILI_INDEX:?}" >>"$FVOCI_FIXTURE_NET_STATE.meili-indexes"
  echo "fixture-scoped-key-${FVOCI_MEILI_INDEX}" >"$2"
fi
exit 0
STUB
cat >"$FIXTURE_ROOT/target/debug/fvoci-server" <<'STUB'
#!/usr/bin/env bash
# FVOCI_FIXTURE_TERM_RESIST=1: the server and its child ignore SIGTERM, so only
# the cleanup's SIGKILL escalation stops them.
if [[ "${FVOCI_FIXTURE_TERM_RESIST:-0}" == 1 ]]; then
  trap '' TERM
fi
# A child of the group server, as the collab engine is: reaped with its group.
if [[ "${FVOCI_FIXTURE_SERVER_CHILD:-0}" == 1 ]]; then
  "$FVOCI_FIXTURE_REAL_SLEEP" 600 &
  echo "$!" >"$FVOCI_FIXTURE_NET_STATE.server-child-pid"
fi
echo "fvoci-server listening on http://127.0.0.1:9"
# Redaction probe: the app-role URL the server does receive (credentials a
# real server must never log), and the admin URL web-e2e-inner.sh withholds
# from the server environment ("unset" only when truly absent).
echo "probe DATABASE_APP_URL=${DATABASE_APP_URL:-} admin ${FVOCI_E2E_ADMIN_DATABASE_URL-unset}"
echo "probe FVOCI_LIBSQL_URL=libsql://libsql-url-secret.example.test FVOCI_LIBSQL_AUTH_TOKEN=libsql-auth-secret FVOCI_TEST_TURSO_DATABASE_URL=https://turso-url-secret.example.test FVOCI_TEST_TURSO_AUTH_TOKEN=turso-auth-secret"
echo "bare libsql://libsql-url-secret.example.test"
echo "bare https://libsql-url-secret.aws-ap-northeast-1.turso.io"
echo "$$" >"$FVOCI_FIXTURE_NET_STATE.server-pid"
if [[ "${FVOCI_FIXTURE_SETUP:-ok}" == "earlydeath" ]]; then
  echo "fixture server exited before setup"
  exit 42
fi
exec sleep 600
STUB
# Only the calls the inner script needs are allowed; anything else fails closed.
# Each psql call's last argument (its SQL) is recorded; FVOCI_FIXTURE_DROP=fail
# fails the database drop.
cat >"$FAKE_BIN/docker" <<'STUB'
#!/usr/bin/env bash
if [[ "${1:-}" == "exec" && "${2:-}" == "-i" && "${3:-}" == "fvoci-fixture-pg" && "${4:-}" == "psql" ]]; then
  sql="${!#}"
  echo "$sql" >>"$FVOCI_FIXTURE_NET_STATE.sql"
  if [[ "$sql" == "DROP DATABASE "* && "${FVOCI_FIXTURE_DROP:-ok}" == fail ]]; then
    echo "fixture: drop refused" >&2
    exit 3
  fi
  exit 0
fi
echo "unexpected docker invocation: $*" >&2
exit 1
STUB
# Netlink monitor and tentative-address query, per FVOCI_FIXTURE_NET:
#   quiet       no tentative address; one monitor event (a redaction probe)
#   tentative-3 the first three queries report a tentative address
#   tentative   every query reports one, so the settle wait hits its bound
#   no-monitor  the monitor exits at once
#   crash       the tentative query fails after the first call (check crashes)
cat >"$FAKE_BIN/ip" <<'STUB'
#!/usr/bin/env bash
mode="${FVOCI_FIXTURE_NET:?}"
if [[ "$*" == "-o -tshort monitor address link" ]]; then
  if [[ "$mode" == "no-monitor" ]]; then
    echo "fixture: netlink unavailable" >&2
    exit 2
  fi
  echo "$$" >"$FVOCI_FIXTURE_NET_STATE.monitor-pid"
  echo "[2026-01-01T00:00:00.000000] 7: veth-fixture@if2: <UP,LOWER_UP> probe postgres://u:fixture-secret@h/db"
  exec sleep 600
fi
if [[ "$*" == "-6 -o addr show tentative -dadfailed" ]]; then
  calls=$(($(cat "$FVOCI_FIXTURE_NET_STATE.calls" 2>/dev/null || echo 0) + 1))
  echo "$calls" >"$FVOCI_FIXTURE_NET_STATE.calls"
  if [[ "$mode" == "crash" ]] && ((calls > 1)); then
    echo "fixture: netlink query failed" >&2
    exit 1
  fi
  if [[ "$mode" == "tentative" ]] || { [[ "$mode" == "tentative-3" ]] && ((calls <= 3)); }; then
    echo "9: veth-fixture    inet6 fe80::1/64 scope link tentative \\       valid_lft forever preferred_lft forever"
  fi
  exit 0
fi
echo "unexpected ip invocation: $*" >&2
exit 1
STUB
cat >"$FAKE_BIN/curl" <<'STUB'
#!/usr/bin/env bash
if [[ "$*" == "-fsS http://127.0.0.1:9/api/v1/setup" ]]; then
  calls=$(($(cat "$FVOCI_FIXTURE_NET_STATE.setup-calls" 2>/dev/null || echo 0) + 1))
  echo "$calls" >"$FVOCI_FIXTURE_NET_STATE.setup-calls"
  case "${FVOCI_FIXTURE_SETUP:-ok}" in
    neverhealthy | earlydeath) exit 22 ;;
    latehealthy) ((calls == 120)) || exit 22 ;;
    ok) ;;
    *) echo "unexpected setup mode" >&2; exit 1 ;;
  esac
  echo "$calls" >"$FVOCI_FIXTURE_NET_STATE.setup-success"
  exit 0
fi
# Meilisearch: URL and master key arrive on stdin (-K -), never in argv. Index
# deletion answers 202 with a task, as Meilisearch does; the task is still
# processing on its first poll and then ends per FVOCI_FIXTURE_MEILI_TASK:
#   succeeded  (default) the index is gone (GET answers 404)
#   failed     the task fails and the index stays
#   stuck      the task never ends
#   present    the task succeeds but the index still answers 200
#   absent     the index never existed: the task fails with index_not_found
#              and GET answers 404
if [[ "$*" == "-sS --max-time 30 -X "@(DELETE|GET)" -w \\n%{http_code} -K -" ]]; then
  method="$5"
  config="$(cat)"
  url="$(sed -n 's/^url = "\(.*\)"$/\1/p' <<<"$config")"
  [[ "$config" == *'header = "Authorization: Bearer fixture-meili-master-secret"'* ]] || {
    echo "fixture: Meilisearch request without the master key" >&2
    exit 1
  }
  path="${url#http://127.0.0.1:7}"
  task_mode="${FVOCI_FIXTURE_MEILI_TASK:-succeeded}"
  body=""
  case "$method $path" in
    "DELETE /indexes/"*)
      echo "DELETE $path" >>"$FVOCI_FIXTURE_NET_STATE.meili-deletes"
      code=202
      body="{\"taskUid\":17,\"indexUid\":\"${path#/indexes/}\",\"status\":\"enqueued\",\"type\":\"indexDeletion\"}"
      ;;
    "DELETE /keys/"*)
      echo "DELETE $path" >>"$FVOCI_FIXTURE_NET_STATE.meili-deletes"
      code=204
      ;;
    "GET /tasks/17")
      echo poll >>"$FVOCI_FIXTURE_NET_STATE.meili-task-polls"
      status=processing
      if (($(wc -l <"$FVOCI_FIXTURE_NET_STATE.meili-task-polls") > 1)); then
        case "$task_mode" in
          succeeded | present) status=succeeded ;;
          failed | absent) status=failed ;;
          stuck) ;;
          *) echo "fixture: unknown Meilisearch task mode" >&2; exit 1 ;;
        esac
      fi
      code=200
      body="{\"uid\":17,\"indexUid\":\"x\",\"status\":\"$status\",\"type\":\"indexDeletion\""
      if [[ "$task_mode" == absent && "$status" == failed ]]; then
        body+=",\"error\":{\"message\":\"Index not found.\",\"code\":\"index_not_found\",\"type\":\"invalid_request\"}"
      elif [[ "$status" == failed ]]; then
        body+=",\"error\":{\"message\":\"fixture\",\"code\":\"internal\",\"type\":\"internal\"}"
      fi
      body+="}"
      ;;
    "GET /indexes/"*)
      echo "GET $path" >>"$FVOCI_FIXTURE_NET_STATE.meili-index-gets"
      if [[ "$task_mode" == succeeded || "$task_mode" == absent ]]; then
        code=404
        body="{\"message\":\"Index not found.\",\"code\":\"index_not_found\",\"type\":\"invalid_request\"}"
      else
        code=200
        body="{\"uid\":\"${path#/indexes/}\"}"
      fi
      ;;
    *)
      echo "unexpected Meilisearch request: $method $path" >&2
      exit 1
      ;;
  esac
  printf '%s\n%s' "$body" "$code"
  exit 0
fi
echo "unexpected curl invocation: $*" >&2
exit 1
STUB
# Only startup polling sleeps are accelerated in readiness cases. The actual
# wrappers still execute all 120 polls with their unchanged production limits.
cat >"$FAKE_BIN/sleep" <<'STUB'
#!/usr/bin/env bash
if [[ "$*" == "0.25" && "${FVOCI_FIXTURE_SETUP:-ok}" != "ok" ]]; then
  exit 0
fi
# The Meilisearch task wait keeps its production poll count; only its pause is
# skipped when the fixture task never ends.
if [[ "$*" == "0.2" && "${FVOCI_FIXTURE_MEILI_TASK:-}" == stuck ]]; then
  exit 0
fi
exec "$FVOCI_FIXTURE_REAL_SLEEP" "$@"
STUB
# Synchronize the controlled listening line before the real startup loop. A
# background stub otherwise races the first poll, making final-poll health
# depend on scheduling rather than on the readiness contract under test.
cat >"$FAKE_BIN/seq" <<'STUB'
#!/usr/bin/env bash
if [[ "$*" == "1 120" && "${FVOCI_FIXTURE_SETUP:-ok}" != "ok" ]]; then
  for _ in {1..100}; do
    [[ ! -f "$FVOCI_FIXTURE_NET_STATE.server-pid" ]] || exec "$FVOCI_FIXTURE_REAL_SEQ" "$@"
    "$FVOCI_FIXTURE_REAL_SLEEP" 0.01
  done
  echo "fixture server did not publish its listening line" >&2
  exit 1
fi
exec "$FVOCI_FIXTURE_REAL_SEQ" "$@"
STUB
# Ordinary readiness cases enter the real page-less Playwright CLI. Perf only
# records entry: its real specs require a database and browser outside this test.
# The SMTP sink records its pid (it must be reaped); FVOCI_FIXTURE_TRACE_SUMMARY=fail
# makes the trace summarizer fail with credential-bearing error text, and
# FVOCI_FIXTURE_PLAYWRIGHT_EXIT=<n> replaces the Playwright run by that exit.
cat >"$FAKE_BIN/bun" <<'STUB'
#!/usr/bin/env bash
case "${1:-}" in
  */tools/web-e2e/smtp-sink.ts) echo "$$" >"$FVOCI_FIXTURE_NET_STATE.smtp-pid" ;;
  */tools/web-e2e/trace-summary.ts)
    if [[ "${FVOCI_FIXTURE_TRACE_SUMMARY:-ok}" == fail ]]; then
      echo "fixture summarizer error postgres://u:fixture-secret@h/db MEILI_MASTER_KEY=fixture-meili-master-secret" >&2
      exit 3
    fi
    ;;
esac
if [[ "${1:-}" == "--bun" && "${2:-}" == "x" && "${3:-}" == "--no-install" && "${4:-}" == "playwright" && "${5:-}" == "test" ]]; then
  if [[ "${FVOCI_FIXTURE_RECORD_PLAYWRIGHT:-0}" == "1" ]]; then
    echo "launch" >>"$FVOCI_FIXTURE_NET_STATE.playwright-launches"
    if [[ "${6:-}" == "--config=e2e/perf/perf.config.ts" ]]; then
      exit 0
    fi
  fi
  if [[ -n "${FVOCI_FIXTURE_PLAYWRIGHT_EXIT:-}" ]]; then
    exit "$FVOCI_FIXTURE_PLAYWRIGHT_EXIT"
  fi
fi
exec "$FVOCI_FIXTURE_REAL_BUN" "$@"
STUB
chmod +x "$FIXTURE_ROOT"/scripts/*.sh "$FIXTURE_ROOT/target/debug/"* "$FAKE_BIN/"*

# run_group <pending:0|1> <outcome:pass|fail> <net-mode> <log> <github-output>
run_group() {
  local pending="$1" outcome="$2" net_mode="$3" log="$4" gh_output="$5"
  local args=()
  if [[ "$pending" == "0" ]]; then
    args=(e2e/controlled-failure.spec.ts)
  fi
  : >"$gh_output"
  rm -f "$NET_STATE".*
  (
    export PATH="$FIXTURE_PATH"
    export TMPDIR="$RUN_TMP"
    export ROOT="$FIXTURE_ROOT"
    export CARGO_TARGET_DIR="$FIXTURE_ROOT/target"
    export GITHUB_OUTPUT="$gh_output"
    export FVOCI_FIXTURE_OUTCOME="$outcome"
    export FVOCI_FIXTURE_NET="$net_mode"
    export FVOCI_FIXTURE_NET_STATE="$NET_STATE"
    export FVOCI_FIXTURE_REAL_BUN="$REAL_BUN"
    export FVOCI_FIXTURE_REAL_SLEEP="$REAL_SLEEP"
    export FVOCI_FIXTURE_REAL_SEQ="$REAL_SEQ"
    export FVOCI_FIXTURE_SERVER_CHILD=1
    if [[ "$pending" == "1" ]]; then
      export FVOCI_E2E_PENDING=1
    else
      unset FVOCI_E2E_PENDING
    fi
    cd "$FIXTURE_ROOT"
    bash scripts/web-e2e-run-group.sh "${args[@]}"
  ) >"$log" 2>&1
}

fail() {
  echo "failure-output fixture: $1" >&2
  [[ -n "${2:-}" ]] && cat "$2" >&2
  exit 1
}

# The group stopped the (fake) netlink monitor it started.
check_monitor_stopped() {
  local label="$1" log="$2" pid
  [[ -f "$NET_STATE.monitor-pid" ]] || return 0
  pid="$(cat "$NET_STATE.monitor-pid")"
  ! kill -0 "$pid" 2>/dev/null || fail "$label: netlink monitor $pid still running" "$log"
}

# line_of <file> <fixed string>: first matching line number, or fail.
line_of() {
  grep -n -F -m1 -- "$2" "$1" | cut -d: -f1 | grep . || fail "missing '$2' in $1" "$1"
}

# check_retired <label> <log>: every database the run created was dropped with
# its app role, and the server, the server's own child and the SMTP sink are
# gone once the group has exited.
check_retired() {
  local label="$1" log="$2" name pid_file pid
  local -a created=()
  if [[ -f "$NET_STATE.sql" ]]; then
    mapfile -t created < <(sed -n 's/^CREATE DATABASE "\(.*\)"$/\1/p' "$NET_STATE.sql")
  fi
  for name in "${created[@]}"; do
    grep -qxF "DROP DATABASE IF EXISTS \"$name\" WITH (FORCE)" "$NET_STATE.sql" \
      || fail "$label: database $name was not dropped" "$log"
    grep -qxF "DROP ROLE IF EXISTS \"fvoci_app_$name\"" "$NET_STATE.sql" \
      || fail "$label: app role of $name was not dropped" "$log"
  done
  for pid_file in server-pid server-child-pid smtp-pid spec-child-pid; do
    [[ -f "$NET_STATE.$pid_file" ]] || continue
    pid="$(cat "$NET_STATE.$pid_file")"
    ! kill -0 "$pid" 2>/dev/null || fail "$label: $pid_file $pid still running" "$log"
  done
}

# pending: 0 ordinary, 1 pending; each failing run keeps a monitor, the ordinary
# pass run never settles (bounded wait) and the pending pass run has no monitor.
for pending in 0 1; do
  label="ordinary"
  fail_net=quiet
  pass_net=tentative
  if [[ "$pending" == "1" ]]; then
    label="pending"
    fail_net=tentative-3
    pass_net=no-monitor
  fi
  log="$WORK/$label-fail.log"
  gh_output="$WORK/$label-fail.github-output"

  status=0
  run_group "$pending" fail "$fail_net" "$log" "$gh_output" || status=$?
  ((status != 0)) || fail "$label: controlled failure exited 0" "$log"
  grep -q '1 failed' "$log" || fail "$label: expected 1 failed test" "$log"
  grep -q '1 passed' "$log" || fail "$label: expected 1 passed test" "$log"

  retained="$(sed -n 's/^failure-artifacts=//p' "$gh_output")"
  [[ -n "$retained" && -d "$retained" ]] || fail "$label: no failure-artifacts output" "$log"
  # Same selection as the upload-artifact path in .github/workflows/web.yml.
  mapfile -t contexts < <(find "$retained/playwright-output" -name error-context.md -type f 2>/dev/null)
  ((${#contexts[@]} == 1)) || fail "$label: expected 1 retained error-context.md, got ${#contexts[@]}" "$log"
  grep -q 'controlled failure' "${contexts[0]}" || fail "$label: error-context.md lacks the failing test" "$log"
  [[ "$(stat -c %a "$retained")" == "700" ]] || fail "$label: retained dir is not private" "$log"
  mapfile -t summaries < <(find "$retained/playwright-output" -name browser-summary.txt -type f 2>/dev/null)
  ((${#summaries[@]} == 1)) || fail "$label: expected 1 browser-summary.txt, got ${#summaries[@]}" "$log"
  [[ "$(head -n1 "${summaries[0]}")" == "browser summary: "* ]] || fail "$label: browser-summary.txt is not a summary" "$log"
  if [[ "$pending" == "0" ]]; then
    [[ -f "$retained/server.log" ]] || fail "$label: the group server.log was not retained" "$log"
    grep -qx 'probe DATABASE_APP_URL=redacted admin unset' "$retained/server.log" \
      || fail "$label: redaction probe missing from server.log" "$log"
    grep -q 'FVOCI_LIBSQL_URL=redacted' "$retained/server.log" \
      || fail "$label: libsql url not redacted in server.log" "$log"
    grep -q 'FVOCI_LIBSQL_AUTH_TOKEN=redacted' "$retained/server.log" \
      || fail "$label: libsql token not redacted in server.log" "$log"
    grep -q 'FVOCI_TEST_TURSO_DATABASE_URL=redacted' "$retained/server.log" \
      || fail "$label: test turso url not redacted in server.log" "$log"
    grep -q 'FVOCI_TEST_TURSO_AUTH_TOKEN=redacted' "$retained/server.log" \
      || fail "$label: test turso token not redacted in server.log" "$log"
    grep -q 'libsql://redacted' "$retained/server.log" \
      || fail "$label: bare libsql url not redacted in server.log" "$log"
    grep -q 'https://redacted' "$retained/server.log" \
      || fail "$label: bare turso https url not redacted in server.log" "$log"
    ! grep -q -e 'fixture-secret' -e 'libsql-url-secret' -e 'libsql-auth-secret' -e 'turso-url-secret' -e 'turso-auth-secret' -e '://[^/[:space:]]*:[^@[:space:]]*@' "$retained/server.log" || fail "$label: credentials in retained server.log" "$log"
  fi
  for shared in test-results test-results-collab e2e-pending/test-results-collab; do
    [[ ! -e "$FIXTURE_ROOT/apps/web/$shared" ]] || fail "$label: wrote shared $shared" "$log"
  done
  # Netlink event log: retained, redacted, and one timeline with the markers.
  net="$retained/net-events.log"
  [[ -f "$net" ]] || fail "$label: net-events.log was not retained" "$log"
  grep -q 'veth-fixture@if2: <UP,LOWER_UP> probe postgres://redacted$' "$net" \
    || fail "$label: monitor event or its redaction missing from net-events.log" "$net"
  ! grep -q 'fixture-secret' "$net" || fail "$label: credentials in retained net-events.log" "$net"
  containers="$(line_of "$net" '# fvoci: starting test containers')"
  settled="$(line_of "$net" '# fvoci: network settle: settled after')"
  browser="$(line_of "$net" '# fvoci: playwright start')"
  exited="$(line_of "$net" '# fvoci: playwright exited with status 1')"
  ((containers < settled && settled < browser && browser < exited)) \
    || fail "$label: net-events.log markers out of order" "$net"
  grep -q '^network settle: settled after [0-9.]* s; netlink events since the group started: 1$' "$log" \
    || fail "$label: settle result not reported" "$log"
  grep -q '^network: netlink address/link events while Playwright ran: 0$' "$log" \
    || fail "$label: event count not reported" "$log"
  if [[ "$fail_net" == "tentative-3" ]]; then
    (($(cat "$NET_STATE.calls") >= 4)) || fail "$label: settle did not wait out tentative addresses" "$log"
  fi
  check_monitor_stopped "$label" "$log"
  check_retired "$label" "$log"
  rm -rf "$retained"

  log="$WORK/$label-pass.log"
  gh_output="$WORK/$label-pass.github-output"
  run_group "$pending" pass "$pass_net" "$log" "$gh_output" || fail "$label: passing run failed" "$log"
  grep -q '2 passed' "$log" || fail "$label: expected 2 passed tests" "$log"
  [[ ! -s "$gh_output" ]] || fail "$label: passing run retained artifacts" "$log"
  if [[ "$pass_net" == "tentative" ]]; then
    grep -q '^network settle: warning: host network still changing after 10 s (tentative: veth-fixture; last netlink event [0-9.]* s ago; netlink events since the group started: 1); continuing$' "$log" \
      || fail "$label: bounded settle warning missing" "$log"
  else
    grep -q '^network settle: netlink monitor not running; not checking for recent events$' "$log" \
      || fail "$label: missing-monitor fallback not reported" "$log"
    grep -q '^network settle: settled after [0-9.]* s$' "$log" \
      || fail "$label: settle without a monitor did not return" "$log"
    ! grep -q 'while Playwright ran' "$log" || fail "$label: event count without a monitor" "$log"
  fi
  check_monitor_stopped "$label" "$log"
  check_retired "$label" "$log"
done

# The real startup paths run against controlled listening/health/process
# outcomes. No database, browser or production server is required.
run_perf() {
  local run_dir="$1" log="$2"
  mkdir -p "$run_dir/out" "$run_dir/static"
  rm -f "$NET_STATE".*
  (
    unset FVOCI_PERF_GREP FVOCI_PERF_TAG FVOCI_PERF_KEEP_SERVER_LOG
    env PATH="$FIXTURE_PATH" ROOT="$FIXTURE_ROOT" RUN_DIR="$run_dir" \
      RELEASE="$FIXTURE_ROOT/target/debug" FVOCI_TEST_PG_CONTAINER=fvoci-fixture-pg \
      TEST_DATABASE_URL="postgres://postgres:fixture-secret@127.0.0.1:5432/postgres" \
      FVOCI_PERF_OUT="$run_dir/out" FVOCI_PERF_DATASET=fixture \
      FVOCI_FIXTURE_NET_STATE="$NET_STATE" FVOCI_FIXTURE_REAL_BUN="$REAL_BUN" \
      FVOCI_FIXTURE_REAL_SLEEP="$REAL_SLEEP" \
      FVOCI_FIXTURE_REAL_SEQ="$REAL_SEQ" \
      bash "$FIXTURE_ROOT/scripts/perf/perf-inner.sh"
  ) >"$log" 2>&1
}

for wrapper in ordinary perf; do
  for mode in neverhealthy earlydeath latehealthy; do
    label="$wrapper-$mode"
    log="$WORK/$label.log"
    gh_output="$WORK/$label.github-output"
    status=0
    (
      export FVOCI_FIXTURE_SETUP="$mode"
      export FVOCI_FIXTURE_RECORD_PLAYWRIGHT=1
      if [[ "$wrapper" == "ordinary" ]]; then
        run_group 0 pass quiet "$log" "$gh_output"
      else
        run_perf "$WORK/$label" "$log"
      fi
    ) || status=$?
    launches=0
    [[ ! -f "$NET_STATE.playwright-launches" ]] || launches="$(wc -l <"$NET_STATE.playwright-launches")"
    probes="$(cat "$NET_STATE.setup-calls" 2>/dev/null || echo 0)"
    if [[ "$mode" == "latehealthy" ]]; then
      ((status == 0)) || fail "$label: late health failed" "$log"
      ((probes == 120 && launches == 1)) || fail "$label: expected 120 probes and one Playwright launch" "$log"
      [[ "$(cat "$NET_STATE.setup-success")" == "120" ]] || fail "$label: launched without final-poll health" "$log"
      if [[ "$wrapper" == "ordinary" ]]; then
        grep -q '2 passed' "$log" || fail "$label: page-less Playwright tests did not pass" "$log"
        [[ ! -s "$gh_output" ]] || fail "$label: successful run retained artifacts" "$log"
      else
        [[ -f "$WORK/$label/out/run-fixture.json" ]] || fail "$label: perf did not reach run metadata" "$log"
      fi
    else
      ((status != 0)) || fail "$label: unready server exited 0 (setup probes=$probes, Playwright launches=$launches)" "$log"
      ((launches == 0)) || fail "$label: Playwright entered before readiness" "$log"
      [[ ! -f "$NET_STATE.setup-success" ]] || fail "$label: unexpected health success" "$log"
      if [[ "$mode" == "neverhealthy" ]]; then
        ((probes == 120)) || fail "$label: startup polling bound changed" "$log"
        grep -q '^server did not become ready within 30s (GET /api/v1/setup never succeeded)$' "$log" \
          || fail "$label: missing health failure diagnosis" "$log"
      else
        ((probes < 120)) || fail "$label: did not stop on early exit" "$log"
        grep -qx 'server exited during startup' "$log" || fail "$label: missing early-exit diagnosis" "$log"
        grep -qx 'fixture server exited before setup' "$log" || fail "$label: missing server failure log" "$log"
      fi
      grep -q '^fvoci-server listening on ' "$log" || fail "$label: missing server startup log" "$log"
      ! grep -q -e 'fixture-secret' -e 'libsql-url-secret' -e 'libsql-auth-secret' -e 'turso-url-secret' -e 'turso-auth-secret' "$log" || fail "$label: credentials in startup diagnostics" "$log"
      grep -q 'FVOCI_LIBSQL_URL=redacted' "$log" || fail "$label: startup log left libsql url" "$log"
      grep -q 'FVOCI_LIBSQL_AUTH_TOKEN=redacted' "$log" || fail "$label: startup log left libsql token" "$log"
      grep -q 'FVOCI_TEST_TURSO_DATABASE_URL=redacted' "$log" || fail "$label: startup log left test turso url" "$log"
      grep -q 'FVOCI_TEST_TURSO_AUTH_TOKEN=redacted' "$log" || fail "$label: startup log left test turso token" "$log"
      grep -q 'libsql://redacted' "$log" || fail "$label: startup log left bare libsql url" "$log"
      grep -q 'https://redacted' "$log" || fail "$label: startup log left bare turso https url" "$log"
      if [[ "$wrapper" == "ordinary" ]]; then
        retained="$(sed -n 's/^failure-artifacts=//p' "$gh_output")"
        [[ -n "$retained" && -f "$retained/server.log" ]] || fail "$label: server log not retained" "$log"
        grep -qx 'probe DATABASE_APP_URL=redacted admin unset' "$retained/server.log" \
          || fail "$label: retained server log missing redacted probe" "$log"
        grep -q 'FVOCI_LIBSQL_URL=redacted' "$retained/server.log" \
          || fail "$label: retained server log left libsql url" "$log"
        grep -q 'FVOCI_TEST_TURSO_AUTH_TOKEN=redacted' "$retained/server.log" \
          || fail "$label: retained server log left test turso token" "$log"
        ! grep -q -e 'libsql-url-secret' -e 'libsql-auth-secret' -e 'turso-url-secret' -e 'turso-auth-secret' "$retained/server.log" \
          || fail "$label: libsql secret in retained server.log" "$log"
        ! grep -q '# fvoci: playwright start' "$retained/net-events.log" || fail "$label: browser marker before readiness" "$log"
        [[ ! -d "$retained/playwright-output" ]] || fail "$label: unexpected Playwright output" "$log"
        rm -rf "$retained"
      else
        [[ ! -f "$WORK/$label/out/run-fixture.json" ]] || fail "$label: perf metadata before readiness" "$log"
      fi
    fi
    check_monitor_stopped "$label" "$log"
    if [[ "$wrapper" == "ordinary" ]]; then
      check_retired "$label" "$log"
    fi
    [[ -f "$NET_STATE.server-pid" ]] || fail "$label: server never started" "$log"
    ! kill -0 "$(cat "$NET_STATE.server-pid")" 2>/dev/null || fail "$label: owned server still running" "$log"
    echo "readiness fixture: $label status=$status setup-probes=$probes Playwright-launches=$launches"
  done
done

# retained_of <github-output>: the retained directory a failed group named.
retained_of() {
  sed -n 's/^failure-artifacts=//p' "$1"
}

# A failing setup step is named with its own exit status; nothing later runs
# and what was already created is retired.
log="$WORK/step-failure.log"
gh_output="$WORK/step-failure.github-output"
status=0
(
  export FVOCI_FIXTURE_MIGRATE=fail FVOCI_FIXTURE_RECORD_PLAYWRIGHT=1
  run_group 0 pass quiet "$log" "$gh_output"
) || status=$?
((status == 9)) || fail "step failure: expected the migration's exit 9, got $status" "$log"
grep -qx 'web-e2e step migrate failed (exit 9)' "$log" || fail "step failure: failing step not named" "$log"
grep -qx 'fixture migration failure' "$log" || fail "step failure: the step's own error is missing" "$log"
[[ ! -f "$NET_STATE.playwright-launches" ]] || fail "step failure: Playwright entered after a failed step" "$log"
grep -q '^CREATE DATABASE ' "$NET_STATE.sql" || fail "step failure: no database was created" "$log"
check_retired step-failure "$log"
rm -rf "$(retained_of "$gh_output")"

# A settle check that cannot run fails the group before the browser starts.
log="$WORK/settle-crash.log"
gh_output="$WORK/settle-crash.github-output"
status=0
(
  export FVOCI_FIXTURE_RECORD_PLAYWRIGHT=1
  run_group 0 pass crash "$log" "$gh_output"
) || status=$?
((status != 0)) || fail "settle crash: group passed" "$log"
grep -qx "web-e2e step network-settle failed (exit $status)" "$log" || fail "settle crash: failing step not named" "$log"
grep -qx 'network settle: error: ip -6 -o addr show tentative -dadfailed exited with status 1: fixture: netlink query failed' "$log" \
  || fail "settle crash: the check's own error is missing" "$log"
[[ ! -f "$NET_STATE.playwright-launches" ]] || fail "settle crash: Playwright entered" "$log"
check_retired settle-crash "$log"
rm -rf "$(retained_of "$gh_output")"

# A trace summary that fails leaves no placeholder summary, names the step
# with its redacted error text and keeps the test's own failing verdict.
log="$WORK/summary-failure.log"
gh_output="$WORK/summary-failure.github-output"
status=0
(
  export FVOCI_FIXTURE_TRACE_SUMMARY=fail
  run_group 0 fail quiet "$log" "$gh_output"
) || status=$?
((status == 1)) || fail "summary failure: expected Playwright's exit 1, got $status" "$log"
retained="$(retained_of "$gh_output")"
[[ -n "$retained" && -d "$retained" ]] || fail "summary failure: nothing retained" "$log"
[[ -z "$(find "$retained" -name 'browser-summary.*' -print -quit)" ]] || fail "summary failure: placeholder summary written" "$log"
[[ -n "$(find "$retained" -name trace.zip -print -quit)" ]] || fail "summary failure: trace.zip not retained" "$log"
grep -q 'retention step trace-summary (.*/trace.zip, exit 3) failed: fixture summarizer error postgres://redacted MEILI_MASTER_KEY=redacted' "$log" \
  || fail "summary failure: step or redacted error missing" "$log"
! grep -q 'fixture-secret' "$log" || fail "summary failure: credentials in the log" "$log"
grep -q 'failure artifacts are incomplete; the group verdict stays exit 1$' "$log" \
  || fail "summary failure: verdict precedence not reported" "$log"
check_retired summary-failure "$log"
rm -rf "$retained"

# A fixture that cannot be retired fails a passing group, but never replaces a
# failing test's own exit status.
log="$WORK/drop-failure-pass.log"
gh_output="$WORK/drop-failure-pass.github-output"
status=0
(
  export FVOCI_FIXTURE_DROP=fail
  run_group 0 pass quiet "$log" "$gh_output"
) || status=$?
((status == 1)) || fail "drop failure: passing group exited $status, expected 1" "$log"
grep -q '2 passed' "$log" || fail "drop failure: tests did not pass" "$log"
grep -qx 'web-e2e cleanup failed: drop-database' "$log" || fail "drop failure: cleanup step not named" "$log"
rm -rf "$(retained_of "$gh_output")"
log="$WORK/drop-failure-exit7.log"
gh_output="$WORK/drop-failure-exit7.github-output"
status=0
(
  export FVOCI_FIXTURE_DROP=fail FVOCI_FIXTURE_PLAYWRIGHT_EXIT=7
  run_group 0 pass quiet "$log" "$gh_output"
) || status=$?
((status == 7)) || fail "drop failure: Playwright exit 7 became $status" "$log"
grep -qx 'web-e2e step playwright failed (exit 7)' "$log" || fail "drop failure: Playwright verdict not named" "$log"
grep -qx 'web-e2e: the group verdict stays exit 7 (step playwright)' "$log" \
  || fail "drop failure: precedence not reported" "$log"
rm -rf "$(retained_of "$gh_output")"

# The group's Meilisearch index and its scoped key are the group's own and
# are deleted; the master key reaches curl only on stdin.
log="$WORK/meili.log"
gh_output="$WORK/meili.github-output"
(
  export FVOCI_FIXTURE_MEILI=1
  run_group 0 pass quiet "$log" "$gh_output"
) || fail "meili: passing run failed" "$log"
index="$(cat "$NET_STATE.meili-indexes")"
[[ "$index" =~ ^fvoci_e2e_[0-9a-f]{16}$ ]] || fail "meili: unexpected group index '$index'" "$log"
db="$(sed -n 's/^CREATE DATABASE "\(.*\)"$/\1/p' "$NET_STATE.sql")"
[[ "$db" == "$index" ]] || fail "meili: index $index does not match database $db" "$log"
[[ "$(cat "$NET_STATE.meili-deletes")" == "DELETE /indexes/$index"$'\n'"DELETE /keys/fixture-scoped-key-$index" ]] \
  || fail "meili: index or key not deleted" "$NET_STATE.meili-deletes"
# HTTP 202 only enqueues the deletion: the task was polled past "processing"
# and the index was confirmed absent.
[[ "$(wc -l <"$NET_STATE.meili-task-polls")" == 2 ]] || fail "meili: deletion task not awaited" "$log"
[[ "$(cat "$NET_STATE.meili-index-gets")" == "GET /indexes/$index" ]] || fail "meili: index absence not checked" "$log"
! grep -q 'fixture-meili-master-secret' "$log" || fail "meili: master key in the log" "$log"
check_retired meili "$log"

# A deletion task that fails, never ends, or leaves the index behind is a
# cleanup failure: it fails a passing group and never replaces a failing
# test's own exit status. The scoped key is still deleted. An index that never
# existed (setup stopped before --ensure-meili-key created it) fails its task
# with index_not_found; with GET answering 404 that is a completed retirement.
for task_mode in failed stuck present failed-exit7 absent; do
  log="$WORK/meili-$task_mode.log"
  gh_output="$WORK/meili-$task_mode.github-output"
  expected=1
  status=0
  (
    export FVOCI_FIXTURE_MEILI=1 FVOCI_FIXTURE_MEILI_TASK="${task_mode%-exit7}"
    if [[ "$task_mode" == *-exit7 ]]; then
      export FVOCI_FIXTURE_PLAYWRIGHT_EXIT=7
    fi
    run_group 0 pass quiet "$log" "$gh_output"
  ) || status=$?
  case "$task_mode" in
    *-exit7) expected=7 ;;
    absent) expected=0 ;;
  esac
  ((status == expected)) || fail "meili $task_mode: expected exit $expected, got $status" "$log"
  if [[ "$task_mode" == absent ]]; then
    ! grep -q '^web-e2e cleanup failed' "$log" || fail "meili absent: a missing index failed the cleanup" "$log"
  else
    grep -qx 'web-e2e cleanup failed: delete-meili-index' "$log" || fail "meili $task_mode: cleanup step not named" "$log"
  fi
  if [[ "$task_mode" == *-exit7 ]]; then
    grep -qx 'web-e2e: the group verdict stays exit 7 (step playwright)' "$log" \
      || fail "meili $task_mode: precedence not reported" "$log"
  fi
  index="$(cat "$NET_STATE.meili-indexes")"
  grep -qxF "DELETE /keys/fixture-scoped-key-$index" "$NET_STATE.meili-deletes" \
    || fail "meili $task_mode: scoped key not deleted" "$log"
  polls="$(wc -l <"$NET_STATE.meili-task-polls")"
  case "$task_mode" in
    # Bounded by both the poll count and the wall-clock deadline.
    stuck)
      ((polls >= 2 && polls <= 300)) || fail "meili stuck: expected 2..300 task polls, got $polls" "$log"
      grep -q 'Meilisearch index deletion task 17 did not succeed (status processing)$' "$log" \
        || fail "meili stuck: unfinished task not named" "$log"
      ;;
    *) ((polls == 2)) || fail "meili $task_mode: expected 2 task polls, got $polls" "$log" ;;
  esac
  ! grep -q 'fixture-meili-master-secret' "$log" || fail "meili $task_mode: master key in the log" "$log"
  check_retired "meili-$task_mode" "$log"
  rm -rf "$(retained_of "$gh_output")"
done

# Services shared by one scope: started once, each group creates and retires
# its own database, role and index.
log="$WORK/shared.log"
rm -f "$NET_STATE".*
(
  export PATH="$FIXTURE_PATH" TMPDIR="$RUN_TMP" ROOT="$FIXTURE_ROOT" CARGO_TARGET_DIR="$FIXTURE_ROOT/target"
  export FVOCI_FIXTURE_OUTCOME=pass FVOCI_FIXTURE_NET=quiet FVOCI_FIXTURE_NET_STATE="$NET_STATE"
  export FVOCI_FIXTURE_REAL_BUN="$REAL_BUN" FVOCI_FIXTURE_REAL_SLEEP="$REAL_SLEEP" FVOCI_FIXTURE_REAL_SEQ="$REAL_SEQ"
  export FVOCI_FIXTURE_SERVER_CHILD=1 FVOCI_FIXTURE_MEILI=1
  unset FVOCI_E2E_PENDING GITHUB_OUTPUT
  cd "$FIXTURE_ROOT"
  bash scripts/start-test-postgres.sh bash scripts/start-test-meili.sh \
    env FVOCI_E2E_SHARED_SERVICES=1 bash -c \
    'bash scripts/web-e2e-run-group.sh e2e/controlled-failure.spec.ts && bash scripts/web-e2e-run-group.sh e2e/controlled-failure.spec.ts'
) >"$log" 2>&1 || fail "shared: groups failed" "$log"
[[ "$(sort "$NET_STATE.service-starts" | tr '\n' ' ')" == "meili postgres " ]] || fail "shared: services started more than once" "$log"
[[ "$(grep -c '^CREATE DATABASE ' "$NET_STATE.sql")" == 2 ]] || fail "shared: expected two group databases" "$log"
[[ "$(sort -u "$NET_STATE.meili-indexes" | wc -l)" == 2 ]] || fail "shared: groups did not get distinct indexes" "$log"
[[ "$(grep -c '^DELETE /indexes/' "$NET_STATE.meili-deletes")" == 2 ]] || fail "shared: indexes not deleted" "$log"
check_retired shared "$log"
log="$WORK/shared-missing.log"
gh_output="$WORK/shared-missing.github-output"
: >"$gh_output"
rm -f "$NET_STATE".*
status=0
(
  export PATH="$FIXTURE_PATH" TMPDIR="$RUN_TMP" ROOT="$FIXTURE_ROOT" CARGO_TARGET_DIR="$FIXTURE_ROOT/target"
  export FVOCI_FIXTURE_NET=quiet FVOCI_FIXTURE_NET_STATE="$NET_STATE" FVOCI_E2E_SHARED_SERVICES=1
  export GITHUB_OUTPUT="$gh_output"
  unset TEST_DATABASE_URL FVOCI_TEST_PG_CONTAINER
  cd "$FIXTURE_ROOT"
  bash scripts/web-e2e-run-group.sh e2e/controlled-failure.spec.ts
) >"$log" 2>&1 || status=$?
((status == 1)) || fail "shared missing: expected exit 1, got $status" "$log"
grep -qx 'FVOCI_E2E_SHARED_SERVICES=1 requires TEST_DATABASE_URL from the scope that started the services' "$log" \
  || fail "shared missing: missing service not named" "$log"
[[ ! -e "$NET_STATE.service-starts" && ! -e "$NET_STATE.sql" ]] || fail "shared missing: services or database touched" "$log"
rm -rf "$(retained_of "$gh_output")"

# SIGINT while Playwright runs (a Ctrl-C reaches the whole process group):
# exit 130, and the database, server, its child and the SMTP sink are retired.
log="$WORK/sigint.log"
gh_output="$WORK/sigint.github-output"
set -m
run_group 0 hang quiet "$log" "$gh_output" &
group_pid=$!
set +m
for _ in {1..600}; do
  [[ ! -f "$NET_STATE.hanging" ]] || break
  kill -0 "$group_pid" 2>/dev/null || break
  "$REAL_SLEEP" 0.1
done
[[ -f "$NET_STATE.hanging" ]] || fail "sigint: Playwright never reached the hanging test" "$log"
kill -INT -- "-$group_pid"
status=0
wait "$group_pid" || status=$?
((status == 130)) || fail "sigint: expected exit 130, got $status" "$log"
[[ -f "$NET_STATE.spec-child-pid" ]] || fail "sigint: the spec child never started" "$log"
check_retired sigint "$log"
grep -q '^DROP DATABASE ' "$NET_STATE.sql" || fail "sigint: database not dropped" "$log"
check_monitor_stopped sigint "$log"
rm -rf "$(retained_of "$gh_output")"

# SIGTERM to the group's process group while its output goes through a reader
# in that same group (a tee, as a CI step log or terminal pipeline is): the
# reader dies with the group, so each diagnostic the cleanup writes raises
# SIGPIPE. With a server and a spec child that ignore SIGTERM, the cleanup
# must still escalate to SIGKILL and retire the database, the app role, the
# Meilisearch index and key, the server, its child and the SMTP sink. The
# services are shared exports, so the real wrappers alone handle the signal.
log="$WORK/sigterm-closed-output.log"
gh_output="$WORK/sigterm-closed-output.github-output"
: >"$gh_output"
rm -f "$NET_STATE".*
set -m
(
  exec > >(exec tee "$log" >/dev/null) 2>&1
  export PATH="$FIXTURE_PATH" TMPDIR="$RUN_TMP" ROOT="$FIXTURE_ROOT" CARGO_TARGET_DIR="$FIXTURE_ROOT/target"
  export GITHUB_OUTPUT="$gh_output"
  export FVOCI_FIXTURE_OUTCOME=hang FVOCI_FIXTURE_NET=quiet FVOCI_FIXTURE_NET_STATE="$NET_STATE"
  export FVOCI_FIXTURE_REAL_BUN="$REAL_BUN" FVOCI_FIXTURE_REAL_SLEEP="$REAL_SLEEP" FVOCI_FIXTURE_REAL_SEQ="$REAL_SEQ"
  export FVOCI_FIXTURE_SERVER_CHILD=1 FVOCI_FIXTURE_TERM_RESIST=1
  export FVOCI_E2E_SHARED_SERVICES=1 FVOCI_TEST_PG_CONTAINER=fvoci-fixture-pg
  export TEST_DATABASE_URL="postgres://postgres:fixture-secret@127.0.0.1:5432/postgres"
  export FVOCI_MEILI_URL="http://127.0.0.1:7" MEILI_MASTER_KEY="fixture-meili-master-secret"
  export FVOCI_MEILI_KEY="$MEILI_MASTER_KEY" FVOCI_TEST_MEILI_CONTAINER=fvoci-fixture-meili
  unset FVOCI_E2E_PENDING
  cd "$FIXTURE_ROOT"
  exec bash scripts/web-e2e-run-group.sh e2e/controlled-failure.spec.ts
) &
group_pid=$!
set +m
for _ in {1..600}; do
  [[ ! -f "$NET_STATE.hanging" || ! -f "$NET_STATE.server-child-pid" ]] || break
  kill -0 "$group_pid" 2>/dev/null || break
  "$REAL_SLEEP" 0.1
done
[[ -f "$NET_STATE.hanging" ]] || fail "sigterm closed output: Playwright never reached the hanging test" "$log"
kill -TERM -- "-$group_pid"
status=0
wait "$group_pid" || status=$?
((status == 143)) || fail "sigterm closed output: expected exit 143, got $status" "$log"
for pid_file in server-pid server-child-pid smtp-pid spec-child-pid; do
  [[ -f "$NET_STATE.$pid_file" ]] || fail "sigterm closed output: $pid_file never recorded" "$log"
done
check_retired sigterm-closed-output "$log"
grep -q '^DROP DATABASE ' "$NET_STATE.sql" || fail "sigterm closed output: database not dropped" "$log"
index="$(cat "$NET_STATE.meili-indexes")"
[[ "$(cat "$NET_STATE.meili-deletes")" == "DELETE /indexes/$index"$'\n'"DELETE /keys/fixture-scoped-key-$index" ]] \
  || fail "sigterm closed output: Meilisearch index or key not deleted" "$log"
check_monitor_stopped sigterm-closed-output "$log"
rm -rf "$(retained_of "$gh_output")"

leftover="$(find "$RUN_TMP" -mindepth 1 -maxdepth 1 -name 'fvoci-*' -print -quit)"
[[ -z "$leftover" ]] || fail "run directory not cleaned: $leftover"

echo "failure-output-fixture-test: ok"
