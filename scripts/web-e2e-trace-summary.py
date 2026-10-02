#!/usr/bin/env python3
"""Digest of one Playwright trace.zip that CI may upload (web-e2e-run-group.sh).

Prints each request's method, status, resource type and path (no headers,
cookies, bodies or query strings), browser console warnings/errors and page
errors. Share, invite, invitation and ICS path tokens, parameters whose key
ends in token/code/secret/password/state/ticket/key, long base64url runs and
postgres URLs are redacted. The raw trace (headers, cookies, DOM, bodies)
stays local. Usage: web-e2e-trace-summary.py <trace.zip>
"""
import hashlib, json, math, re, sys, urllib.parse, zipfile

PATH_TOKEN = re.compile(r"(/(?:s|invite|share|invitations|ics)/)[^/?#\s\"'<>]+")
# The lookbehind anchors a match at the start of its key, which keeps a
# long run of key characters without "=" linear instead of quadratic.
PARAM_TOKEN = re.compile(r"(?i)(?<![\w.-])([\w.-]*(?:token|code|secret|password|state|ticket|key))=[^&\s\"'<>#]+")
LONG_TOKEN = re.compile(r"[A-Za-z0-9_-]{40,}")
DB_URL = re.compile(r"postgres(?:ql)?://\S+")
MAX_LINES = 2000
DIAGNOSTIC_NAME = "w3-template-native-selection-observation.json"
DIAGNOSTIC_ACTIONS = (
    "openDoc:original", "ShiftHome:original-return", "bubble:original-visible",
    "popup:original-open-focus", "compositionEnter:original-no-link",
    "Cancel:original-native-text", "Apply:original-selected-text", "save:original", "finally",
)

def diagnostic_digest(trace):
    """Read this fixture's one named attachment; export only an explicit allowlist.

    Raw trace/attachment content stays on the runner. All invalid-input outcomes
    are fixed enums, never exception messages or a raw-content fallback.
    """
    def unavailable(reason):
        return {"available": False, "reason": reason}

    def number(value):
        return value if type(value) in (int, float) and 0 <= value <= 1e12 and math.isfinite(value) else None

    def integer(value):
        return value if type(value) is int and 0 <= value <= 1_000_000_000 else None

    def boolean(value):
        return value if type(value) is bool else None

    def enum(value, allowed):
        return value if type(value) is str and value in allowed else "unknown"

    def stage(value):
        if value in DIAGNOSTIC_ACTIONS or value in (
            "frame", "provider:authenticated", "provider:status", "provider:synced",
            "selectionchange:", "selectionchange::microtask", "focusin:", "focusin::microtask",
            "focusout:", "focusout::microtask", "pointerdown:", "pointerdown::microtask",
            "pagehide:", "pagehide::microtask",
        ):
            return value
        if type(value) is str and re.fullmatch(r"(?:transaction:doc=(?:true|false):selection=(?:true|false)|Yupdate:local=(?:true|false))", value):
            return value
        if type(value) is str and re.fullmatch(r"(?:keydown|keyup):(?:Home|End|Shift|Enter|Escape|ArrowLeft|ArrowRight|ArrowUp|ArrowDown|PageUp|PageDown):shift=(?:true|false):composing=(?:true|false):keyCode=\d{1,3}(?::microtask)?", value):
            return value
        return "unknown"

    def owner(value):
        if type(value) is not str or len(value) > 256:
            return None
        try:
            ids = json.loads(value)
        except (ValueError, RecursionError):
            return None
        if type(ids) is not list or len(ids) != 7 or any(x is not None and integer(x) is None for x in ids):
            return None
        return ids

    def positions(value):
        value = value if type(value) is dict else {}
        return {"anchor": integer(value.get("anchor")), "head": integer(value.get("head"))}

    def reject_constant(_value):
        raise ValueError("Non-JSON numeric constant")

    members = trace.infolist()
    if len(members) > 4096:
        return unavailable("member_limit")
    named = [item for item in members if item.filename == "test.trace"]
    if len(named) != 1:
        return unavailable("missing_or_duplicate_test_trace")
    if named[0].file_size > 4 * 1024 * 1024:
        return unavailable("test_trace_size_limit")
    try:
        lines = trace.read(named[0]).splitlines()
        if len(lines) > 10000:
            return unavailable("test_event_limit")
        references = []
        for line in lines:
            event = json.loads(line, parse_constant=reject_constant)
            if type(event) is not dict:
                return unavailable("invalid_test_event")
            attachments = event.get("attachments", [])
            if type(attachments) is not list or len(attachments) > 32:
                return unavailable("invalid_attachment_list")
            references.extend(item for item in attachments if type(item) is dict and item.get("name") == DIAGNOSTIC_NAME)
        if len(references) != 1:
            return unavailable("missing_or_duplicate_attachment")
        reference = references[0]
        path = reference.get("file")
        if reference.get("contentType") != "application/json" or type(path) is not str or not re.fullmatch(r"attachments/[a-f0-9]{40,64}", path):
            return unavailable("invalid_attachment_reference")
        matching = [item for item in members if item.filename == path]
        if len(matching) != 1 or matching[0].is_dir() or matching[0].flag_bits & 1 or (matching[0].external_attr >> 16) & 0o170000 not in (0, 0o100000):
            return unavailable("missing_or_invalid_attachment_member")
        if matching[0].file_size > 1024 * 1024:
            return unavailable("attachment_size_limit")
        data = json.loads(trace.read(matching[0]), parse_constant=reject_constant)
    except (ValueError, UnicodeError, RecursionError, RuntimeError, zipfile.BadZipFile, OSError):
        return unavailable("invalid_attachment_data")
    if type(data) is not dict:
        return unavailable("invalid_schema")
    keys = ("frames", "critical", "ownerChanges", "observedMismatches")
    limits = (512, 2048, 2048, 4096)
    if any(type(data.get(key)) is not list or len(data[key]) > limit or any(type(x) is not dict for x in data[key]) for key, limit in zip(keys, limits)):
        return unavailable("invalid_event_schema_or_limit")
    boundaries = data.get("actionBoundaries", [])
    if type(boundaries) is not list or len(boundaries) > 16 or any(type(x) is not dict for x in boundaries):
        return unavailable("invalid_boundary_schema")
    events = [x for key in keys for x in data[key]] + boundaries
    for key in ("firstObservedState", "firstRetiredEvent"):
        if key in data and data[key] is not None:
            if type(data[key]) is not dict:
                return unavailable("invalid_first_state_schema")
            events.append(data[key])
    for event in events:
        if number(event.get("at")) is None or type(event.get("stage")) is not str or len(event["stage"]) > 512:
            return unavailable("invalid_event_schema")
        if any(key in event and owner(event[key]) is None for key in ("owner", "previousOwner")):
            return unavailable("invalid_owner_schema")
        if any(key in event and type(event[key]) is not dict for key in ("native", "pm", "focus", "auth")):
            return unavailable("invalid_snapshot_schema")
        native = event.get("native", {})
        if "text" in native and native["text"] is not None and (type(native["text"]) is not str or len(native["text"]) > 4096):
            return unavailable("invalid_native_text_schema")
        for value in (native.get("positions"), event.get("nativePositions"), event.get("pm")):
            if value is not None and (type(value) is not dict or any(key in value and integer(value[key]) is None for key in ("anchor", "head"))):
                return unavailable("invalid_position_schema")
        for value, fields in ((native, ("inside",)), (event.get("pm", {}), ("empty",)),
                              (event.get("focus", {}), ("editor", "editorEditable", "composing")),
                              (event.get("auth", {}), ("authenticated", "synced"))):
            if any(key in value and value[key] is not None and type(value[key]) is not bool for key in fields):
                return unavailable("invalid_boolean_schema")
    if any(integer(data.get(key)) is None for key in ("updates", "localUpdates")) or data["localUpdates"] > data["updates"]:
        return unavailable("invalid_update_counts")
    totals = data.get("totals", {})
    dropped = data.get("dropped", {})
    if type(totals) is not dict or type(dropped) is not dict:
        return unavailable("invalid_collection_counts")
    counts = {}
    for key in (*keys, "actionBoundaries"):
        retained = len(boundaries if key == "actionBoundaries" else data[key])
        total, lost = integer(totals.get(key)), integer(dropped.get(key))
        if total is not None and lost is not None and total != retained + lost:
            return unavailable("inconsistent_collection_counts")
        counts[key] = {"retained": retained, "total": total, "dropped": lost, "unknown": total is None or lost is None}

    first = data.get("firstObservedState")
    if type(first) is not dict:
        first = next((x for x in data["frames"] if "pm" in x), None)
    baseline_id = first.get("nativeId") if first else None

    def sample(raw):
        if type(raw) is not dict:
            return None
        native = raw.get("native") if type(raw.get("native")) is dict else {}
        pm = raw.get("pm") if type(raw.get("pm")) is dict else {}
        focus = raw.get("focus") if type(raw.get("focus")) is dict else {}
        auth = raw.get("auth") if type(raw.get("auth")) is dict else {}
        text = native.get("text")
        native_id = raw.get("nativeId")
        valid_id = type(native_id) is str and 0 < len(native_id) <= 256
        bubble = raw.get("bubble")
        selection = positions(pm)
        selection.update(empty=boolean(pm.get("empty")), type=enum(pm.get("type"), ("text", "node", "cell", "all")))
        return {
            "at": number(raw.get("at")), "stage": stage(raw.get("stage")),
            "bindingGeneration": integer(raw.get("bindingGeneration")),
            "eventBindingGeneration": integer(raw.get("eventBindingGeneration")), "retiredEvent": boolean(raw.get("retiredEvent")),
            "owner": owner(raw.get("owner")), "previousOwner": owner(raw.get("previousOwner")),
            "unavailable": enum(raw.get("unavailable"), ("missing-editor", "destroyed-editor")) if "unavailable" in raw else None,
            "captureUnknown": "unknown" in raw,
            "native": {"inside": boolean(native.get("inside")), "codePoints": len(text) if type(text) is str else None,
                       "equalsKnownFixtureCjkEmoji": text == "한글과 😀 링크" if type(text) is str else None,
                       "positions": positions(native.get("positions", raw.get("nativePositions"))),
                       "mappingUnknown": type(native.get("positions")) is dict and "unknown" in native["positions"]},
            "pm": selection,
            "focus": {"activeTag": enum(focus.get("activeTag"), ("BODY", "DIV", "INPUT", "BUTTON", "TEXTAREA")),
                      **{k: boolean(focus.get(k)) for k in ("editor", "editorEditable", "composing")},
                      "domEditable": enum(focus.get("domEditable"), ("true", "false", "inherit"))},
            "auth": {**{k: boolean(auth.get(k)) for k in ("authenticated", "synced")},
                     "scope": enum(auth.get("scope"), ("read-write", "readonly")),
                     "status": enum(auth.get("status"), ("connected", "connecting", "disconnected"))},
            "nativeId": {"present": valid_id, "hash": hashlib.sha256(native_id.encode("utf-8", "surrogatepass")).hexdigest()[:16] if valid_id else None,
                         "sameAsFirst": native_id == baseline_id if valid_id and type(baseline_id) is str else None},
            "updates": integer(raw.get("updates")), "localUpdates": integer(raw.get("localUpdates")),
            "generationUpdates": integer(raw.get("generationUpdates")), "generationLocalUpdates": integer(raw.get("generationLocalUpdates")),
            "retiredUpdate": boolean(raw.get("retiredUpdate")),
            "bubble": {"present": type(bubble) is dict, "visibility": enum(bubble.get("visibility"), ("visible", "hidden")) if type(bubble) is dict else None},
            "dialog": boolean(raw.get("dialog")),
        }

    actions = {}
    for action in DIAGNOSTIC_ACTIONS:
        actions[action] = sample(next((x for x in (*boundaries, *data["critical"]) if x.get("stage") == action), None))
    causal = [x for x in data["critical"] if stage(x.get("stage")).startswith(("keydown:Home:", "keyup:Home:", "transaction:", "provider:"))]
    mismatch = next(iter(data["observedMismatches"]), None)
    mismatch_at = number(mismatch.get("at")) if mismatch else None
    home = [i for i, x in enumerate(causal) if stage(x.get("stage")).startswith(("keydown:Home:", "keyup:Home:"))][:4]
    near = [i for i, x in enumerate(causal) if stage(x.get("stage")).startswith("transaction:") and mismatch_at is not None and number(x.get("at")) is not None and abs(x["at"] - mismatch_at) <= 25][:3]
    admission = [i for i, x in enumerate(causal) if stage(x.get("stage")).startswith("provider:")][:2]
    selected = sorted(set(home + near + admission + list(range(min(1, len(causal)))) + list(range(max(0, len(causal) - 2), len(causal)))))
    result = {
        "available": True, "counts": counts,
        "updates": data["updates"], "localUpdates": data["localUpdates"],
        "bindingGeneration": integer(data.get("bindingGeneration")),
        "firstObservedState": sample(first), "actions": actions,
        "firstRetiredEvent": sample(data.get("firstRetiredEvent")),
        "missingActions": [action for action, value in actions.items() if value is None],
        "firstIdentityChange": sample(next(iter(data["ownerChanges"]), None)),
        "firstMismatch": sample(mismatch),
        "causalSamples": [sample(causal[i]) for i in selected], "causalSampleTotal": len(causal), "causalSamplesTruncated": len(causal) > len(selected),
        "tail": [sample(x) for x in data["frames"][-6:]], "tailTruncated": len(data["frames"]) > 6,
        "earlyBindingNotCaptured": True, "navigationContinuity": "current-document-only",
        "captureErrorsRetained": sum("unknown" in x for x in data["critical"]),
    }
    return result if len(json.dumps(result).encode()) <= 49152 else unavailable("output_size_limit")

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
    diagnostic = diagnostic_digest(trace)
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
print("w3-template-diagnostic " + json.dumps(diagnostic, ensure_ascii=True, separators=(",", ":")))
