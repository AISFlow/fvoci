#!/usr/bin/env bash
# Test-only: feed scripts/web-e2e-trace-summary.py a synthetic trace.zip with
# tokens in paths, parameters, console text and page errors, and check that
# none reaches the summary, that diagnostic paths survive, and that a large
# key-like blob is summarized quickly (the parameter regex stays linear).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-trace-summary.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

python3 - "$WORK/trace.zip" <<'PY'
import json, sys, zipfile
share = "S" * 43
bare = "B" * 45
def snap(url, status, failure="", kind="document", t=1.0):
    return {"type": "resource-snapshot", "snapshot": {
        "request": {"method": "GET", "url": url},
        "response": {"status": status, "_failureText": failure},
        "_resourceType": kind, "startedDateTime": "2026-09-29T00:00:00.000Z",
        "time": 12, "_monotonicTime": t}}
network = [
    snap(f"http://127.0.0.1:4000/s/{share}?code=QUERYSECRET", 404),
    snap("http://127.0.0.1:4000/assets/index-DiwrgTda.js", 0, "net::ERR_ABORTED", "script", 1.5),
    # Shorter than LONG_TOKEN: only the path-token rule redacts it.
    snap("http://127.0.0.1:4000/api/v1/invitations/PATHSECRET/accept", 404, kind="fetch", t=1.7),
]
events = [
    {"type": "console", "messageType": "error", "time": 2.0,
     "text": f"fetch failed access_token=PARAMSECRET1 inviteToken=PARAMSECRET2 {bare} postgresql://u:DBSECRET@h/db",
     "location": {"url": "http://127.0.0.1:4000/assets/app.js"}},
    {"type": "event", "method": "pageError", "time": 3.0,
     "params": {"error": {"error": {"name": "Error", "message": "a" * 200000 + " state=PARAMSECRET3"}}}},
]
with zipfile.ZipFile(sys.argv[1], "w") as z:
    z.writestr("0-trace.network", "\n".join(json.dumps(e) for e in network))
    z.writestr("0-trace.trace", "\n".join(json.dumps(e) for e in events))
PY

fail() {
  echo "trace-summary fixture: $1" >&2
  cat "$WORK/summary.txt" >&2 || true
  exit 1
}

timeout 20 python3 "$ROOT/scripts/web-e2e-trace-summary.py" "$WORK/trace.zip" >"$WORK/summary.txt" \
  || fail "summarizer failed or took over 20 s"
[[ "$(head -n1 "$WORK/summary.txt")" == "browser summary: "* ]] || fail "missing header"
for secret in PATHSECRET QUERYSECRET PARAMSECRET1 PARAMSECRET2 PARAMSECRET3 DBSECRET SSSSSSSSSSSSSSSSSSSS BBBBBBBBBBBBBBBBBBBB; do
  ! grep -q "$secret" "$WORK/summary.txt" || fail "$secret leaked"
done
grep -q '/s/<redacted>?…' "$WORK/summary.txt" || fail "share path not summarized"
grep -q '/api/v1/invitations/<redacted>/accept' "$WORK/summary.txt" || fail "invitation path token not redacted"
grep -q 'GET ERR script /assets/index-DiwrgTda.js' "$WORK/summary.txt" || fail "failed asset request not listed"
grep -q 'net::ERR_ABORTED' "$WORK/summary.txt" || fail "failure text missing"
grep -q 'console   error fetch failed' "$WORK/summary.txt" || fail "console error missing"
grep -q 'pageerror' "$WORK/summary.txt" || fail "page error missing"
echo "trace-summary-fixture-test: ok"
