#!/usr/bin/env python3
"""Digest of one Playwright trace.zip that CI may upload (web-e2e-run-group.sh).

Prints each request's method, status, resource type and path (no headers,
cookies, bodies or query strings), browser console warnings/errors and page
errors. Share, invite, invitation and ICS path tokens, parameters whose key
ends in token/code/secret/password/state/ticket/key, long base64url runs and
postgres URLs are redacted. The raw trace (headers, cookies, DOM, bodies)
stays local. Usage: web-e2e-trace-summary.py <trace.zip>
"""
import json, re, sys, urllib.parse, zipfile

PATH_TOKEN = re.compile(r"(/(?:s|invite|share|invitations|ics)/)[^/?#\s\"'<>]+")
# The lookbehind anchors a match at the start of its key, which keeps a
# long run of key characters without "=" linear instead of quadratic.
PARAM_TOKEN = re.compile(r"(?i)(?<![\w.-])([\w.-]*(?:token|code|secret|password|state|ticket|key))=[^&\s\"'<>#]+")
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
