#!/usr/bin/env bash
# Run one isolated web e2e group (fresh DB/app/server/storage per invocation).
set -euo pipefail

: "${ROOT:?ROOT is required}"
: "${CARGO_TARGET_DIR:?CARGO_TARGET_DIR is required}"

GROUP_LABEL="default-suite"
if [[ "${FVOCI_E2E_PENDING:-}" == "1" ]] && (($# < 1)); then
  GROUP_LABEL="collaboration-pending"
fi

if (($# >= 1)); then
  GROUP_LABEL="$(basename "${1%.spec.ts}")"
  if (($# > 1)); then
    GROUP_LABEL="${GROUP_LABEL}+$(basename "${2%.spec.ts}")"
  fi
fi

RUN_DIR="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-web-e2e.XXXXXX")"
SERVER_LOG="$RUN_DIR/server.log"
PEPPER='{"test":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}'

redact_server_log() {
  sed -E \
    -e 's#postgres(ql)?://[^[:space:]]+#postgres://redacted#g' \
    -e 's#(DATABASE_URL|DATABASE_APP_URL|FVOCI_E2E_ADMIN_DATABASE_URL|TEST_DATABASE_URL)=[^[:space:]]+#\1=redacted#g' \
    "$1"
}

# Prints a digest of one Playwright trace.zip that CI may upload: each request's
# method, status, resource type and path (no headers, cookies, bodies or query
# strings), browser console warnings/errors and page errors. Share, invite,
# invitation and ICS path tokens, *token/*code/*secret/*password/*state/
# *ticket/*key parameters and long base64url runs are redacted. The raw trace (headers, cookies, DOM, bodies) stays local.
summarize_trace() {
  python3 - "$1" <<'PY'
import json, re, sys, urllib.parse, zipfile

PATH_TOKEN = re.compile(r"(/(?:s|invite|share|invitations|ics)/)[^/?#\s\"'<>]+")
PARAM_TOKEN = re.compile(r"(?i)([\w.-]*(?:token|code|secret|password|state|ticket|key))=[^&\s\"'<>#]+")
LONG_TOKEN = re.compile(r"[A-Za-z0-9_-]{40,}")
DB_URL = re.compile(r"postgres(?:ql)?://\S+")
MAX_LINES = 2000

def redact(text):
    text = DB_URL.sub("postgres://redacted", text)
    text = PATH_TOKEN.sub(r"\1<redacted>", text)
    text = PARAM_TOKEN.sub(r"\1=<redacted>", text)
    return LONG_TOKEN.sub("<redacted>", text)

def short_url(url):
    try:
        parts = urllib.parse.urlsplit(url)
    except ValueError:
        return "<unparsable url>"
    host = parts.hostname or ""
    prefix = "" if host in ("127.0.0.1", "localhost", "::1", "") else f"{parts.scheme}://{host}"
    path = redact(prefix + (parts.path or "/"))
    if parts.query:
        path += "?…"
    return path[:120]

def clip(text, limit=600):
    text = redact(str(text))
    return text if len(text) <= limit else text[:limit] + "…"

rows = []
with zipfile.ZipFile(sys.argv[1]) as trace:
    names = sorted(trace.namelist())
    for name in names:
        if not name.endswith(".network"):
            continue
        for line in trace.read(name).decode("utf-8", "replace").splitlines():
            try:
                entry = json.loads(line)
            except ValueError:
                continue
            snap = entry.get("snapshot") if entry.get("type") == "resource-snapshot" else None
            if not snap:
                continue
            request, response = snap.get("request", {}), snap.get("response", {})
            status = response.get("status", 0)
            failure = response.get("_failureText") or ""
            wall = snap.get("startedDateTime", "")[11:23]
            rows.append((snap.get("_monotonicTime", 0.0), "request",
                         f"{wall} {request.get('method', '?')} {status if status and status > 0 else 'ERR'} "
                         f"{snap.get('_resourceType', '-')} {short_url(request.get('url', ''))} "
                         f"{snap.get('time', -1):.0f}ms {clip(failure, 120)}".rstrip()))
    for name in names:
        if not name.endswith(".trace") or name == "test.trace":
            continue
        for line in trace.read(name).decode("utf-8", "replace").splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            kind = event.get("type")
            when = event.get("time", 0.0)
            if kind == "console" and event.get("messageType") in ("error", "warning"):
                location = event.get("location") or {}
                where = short_url(location.get("url", "")) if location.get("url") else ""
                rows.append((when, "console", f"{event['messageType']} {clip(event.get('text', ''))} {where}".rstrip()))
            elif kind == "event" and event.get("method") == "pageError":
                params = event.get("params") or {}
                error = (params.get("error") or {}).get("error") or {}
                message = error.get("stack") or f"{error.get('name', 'Error')}: {error.get('message', params.get('error'))}"
                lines = str(message).splitlines()[:12]
                rows.append((when, "pageerror", clip("\n      ".join(lines), 2000)))
            elif kind == "event" and event.get("method") == "crash":
                rows.append((when, "crash", "page crashed"))

print("browser summary: headers, cookies, bodies and query strings omitted; tokens redacted")
print("columns: +ms since first entry, kind, then for requests: wall clock UTC, method, status, type, path, duration")
if not rows:
    print("(no requests, console warnings/errors or page errors recorded)")
base = min((row[0] for row in rows), default=0.0)
ordered = sorted(rows, key=lambda row: row[0])
# The entries just before the failure matter most: keep the tail.
if len(ordered) > MAX_LINES:
    print(f"… {len(ordered) - MAX_LINES} earlier entries omitted")
    ordered = ordered[-MAX_LINES:]
for when, kind, text in ordered:
    print(f"+{when - base:9.1f} {kind:9} {text}")
PY
}

retain_failure_artifacts() {
  local retain_dir log dest trace
  retain_dir="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-collab-e2e-fail.XXXXXX")"
  chmod 700 "$retain_dir"
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
  while IFS= read -r -d '' trace; do
    dest="$(dirname "$trace")/browser-summary.txt"
    # Python's own error text is not redacted, so it never reaches the file.
    if ! summarize_trace "$trace" >"$dest" 2>/dev/null; then
      echo "trace summary failed; reproduce the group locally to inspect its trace.zip" >"$dest"
      echo "could not summarize $(basename "$(dirname "$trace")")/trace.zip" >&2
    fi
  done < <(find "$retain_dir" -name trace.zip -type f -print0 2>/dev/null || true)
  echo "retained failure artifacts for group ${GROUP_LABEL} in $retain_dir" >&2
  if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
    printf 'failure-artifacts=%s\n' "$retain_dir" >>"$GITHUB_OUTPUT"
    printf 'failure-group=%s\n' "$GROUP_LABEL" >>"$GITHUB_OUTPUT"
  fi
}

cleanup() {
  local status=$?
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
bash "$ROOT/scripts/start-test-postgres.sh" \
  bash "$ROOT/scripts/start-test-meili.sh" \
  env RUN_DIR="$RUN_DIR" SERVER_LOG="$SERVER_LOG" PEPPER="$PEPPER" ROOT="$ROOT" \
    CARGO_TARGET_DIR="$CARGO_TARGET_DIR" FVOCI_STATIC_DIR="$RUN_DIR/static" \
  bash "$ROOT/scripts/web-e2e-inner.sh" "$@"
