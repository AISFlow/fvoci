#!/usr/bin/env bash
# Test-only: run the real web-e2e-run-group.sh / web-e2e-inner.sh and the real
# Playwright CLI with the real configs against controlled page-less specs, and
# check that a failure's error-context.md lands in the retained
# playwright-output directory that CI uploads, for ordinary and pending runs,
# with the group's redacted netlink event log, and that the pre-browser network
# settle wait returns, waits for tentative addresses, and stays bounded.
# PostgreSQL, Meilisearch, SMTP, migrations, the server and `ip` are stubbed; no
# browser is launched because the fixture specs never request `page`.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
PLAYWRIGHT_MODULES="$ROOT/apps/web/node_modules"
if [[ ! -x "$PLAYWRIGHT_MODULES/.bin/playwright" ]]; then
  echo "missing $PLAYWRIGHT_MODULES/.bin/playwright; run npm ci --prefix apps/web" >&2
  exit 1
fi

WORK="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-web-e2e-failure-output.XXXXXX")"
cleanup() {
  rm -rf "$WORK"
}
trap cleanup EXIT

FIXTURE_ROOT="$WORK/root"
FAKE_BIN="$WORK/bin"
RUN_TMP="$WORK/tmp"
NET_STATE="$WORK/net"
mkdir -p "$FIXTURE_ROOT/scripts" "$FIXTURE_ROOT/apps/web/e2e" \
  "$FIXTURE_ROOT/apps/web/e2e-pending" "$FIXTURE_ROOT/apps/web/dist" \
  "$FIXTURE_ROOT/target/debug" "$FAKE_BIN" "$RUN_TMP"
cp "$ROOT/scripts/web-e2e-run-group.sh" "$ROOT/scripts/web-e2e-inner.sh" \
  "$ROOT/scripts/web-e2e-trace-summary.py" "$FIXTURE_ROOT/scripts/"
cp "$ROOT/apps/web/playwright.config.ts" "$FIXTURE_ROOT/apps/web/"
cp "$ROOT/apps/web/e2e-pending/collab-playwright.config.ts" "$FIXTURE_ROOT/apps/web/e2e-pending/"
ln -s "$PLAYWRIGHT_MODULES" "$FIXTURE_ROOT/apps/web/node_modules"
echo "fixture" >"$FIXTURE_ROOT/apps/web/dist/index.html"

for dir in e2e e2e-pending; do
  cat >"$FIXTURE_ROOT/apps/web/$dir/controlled-failure.spec.ts" <<'SPEC'
import { expect, test } from "@playwright/test";

test("controlled pass", async () => {
  expect(1).toBe(1);
});

test("controlled failure", async () => {
  expect(process.env.FVOCI_FIXTURE_OUTCOME).toBe("pass");
});
SPEC
done

cat >"$FIXTURE_ROOT/scripts/start-test-postgres.sh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
export TEST_DATABASE_URL="postgres://postgres:fixture-secret@127.0.0.1:5432/postgres"
export FVOCI_TEST_PG_CONTAINER="fvoci-fixture-pg"
"$@"
STUB
cat >"$FIXTURE_ROOT/scripts/start-test-meili.sh" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
"$@"
STUB
cat >"$FIXTURE_ROOT/scripts/smtp-sink.py" <<'STUB'
import argparse, pathlib, time
parser = argparse.ArgumentParser()
parser.add_argument("--capture")
parser.add_argument("--port-file")
args = parser.parse_args()
pathlib.Path(args.port_file).write_text("2525")
time.sleep(600)
STUB
cat >"$FIXTURE_ROOT/target/debug/fvoci-migrate" <<'STUB'
#!/usr/bin/env bash
exit 0
STUB
cat >"$FIXTURE_ROOT/target/debug/fvoci-server" <<'STUB'
#!/usr/bin/env bash
echo "fvoci-server listening on http://127.0.0.1:9"
# Redaction probe: credentials a real server must never log.
echo "probe DATABASE_APP_URL=${DATABASE_APP_URL:-} admin ${FVOCI_E2E_ADMIN_DATABASE_URL:-}"
exec sleep 600
STUB
# Only the calls the inner script needs are allowed; anything else fails closed.
cat >"$FAKE_BIN/docker" <<'STUB'
#!/usr/bin/env bash
if [[ "${1:-}" == "exec" && "${2:-}" == "-i" && "${3:-}" == "fvoci-fixture-pg" && "${4:-}" == "psql" ]]; then
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
  exit 0
fi
echo "unexpected curl invocation: $*" >&2
exit 1
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
    export PATH="$FAKE_BIN:$PATH"
    export TMPDIR="$RUN_TMP"
    export ROOT="$FIXTURE_ROOT"
    export CARGO_TARGET_DIR="$FIXTURE_ROOT/target"
    export GITHUB_OUTPUT="$gh_output"
    export FVOCI_FIXTURE_OUTCOME="$outcome"
    export FVOCI_FIXTURE_NET="$net_mode"
    export FVOCI_FIXTURE_NET_STATE="$NET_STATE"
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
    grep -qx 'probe DATABASE_APP_URL=redacted admin postgres://redacted' "$retained/server.log" \
      || fail "$label: redaction probe missing from server.log" "$log"
    ! grep -q -e 'fixture-secret' -e '://[^/[:space:]]*:[^@[:space:]]*@' "$retained/server.log" || fail "$label: credentials in retained server.log" "$log"
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
done

leftover="$(find "$RUN_TMP" -mindepth 1 -maxdepth 1 -name 'fvoci-*' -print -quit)"
[[ -z "$leftover" ]] || fail "run directory not cleaned: $leftover"

echo "failure-output-fixture-test: ok"
