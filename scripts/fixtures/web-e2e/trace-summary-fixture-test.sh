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

# Independently declared fixture data, not a converted production snapshot.
# The approved CI artifact already includes this stdout/browser-summary file;
# no raw ZIP, document content or arbitrary attachment becomes uploadable.
python3 - "$ROOT/scripts/web-e2e-trace-summary.py" "$WORK" <<'PY'
import copy, json, pathlib, subprocess, sys, zipfile
script, work = sys.argv[1], pathlib.Path(sys.argv[2])
name = "w3-template-native-selection-observation.json"
member = "attachments/" + "a" * 40
secret = "DIAGNOSTIC_PRIVATE_VALUE"
frame = {
    "at": 10, "stage": "ShiftHome:original-return", "owner": "[1,2,1,3,4,5,6]",
    "bindingGeneration": 1, "nativeId": "NATIVE_ID_PRIVATE",
    "native": {"inside": True, "text": "한글과 😀 링크", "positions": {"anchor": 9, "head": 1}},
    "pm": {"anchor": 9, "head": 1, "empty": False, "type": "text", "marks": [{"href": secret}]},
    "focus": {"activeTag": "DIV", "activeLabel": secret, "editor": True, "editorEditable": True,
              "domEditable": "true", "composing": False},
    "auth": {"authenticated": True, "synced": True, "scope": "read-write", "status": "connected", "password": secret},
    "updates": 0, "localUpdates": 0, "generationUpdates": 0, "generationLocalUpdates": 0,
    "bubble": {"visibility": "visible", "opacity": "1"}, "dialog": False,
    "pmDocument": {"type": "doc", "attrs": {"rawFuture": secret}, "text": secret},
    "unknown": "https://private.example/" + secret + "?password=" + secret,
}
actions = ["openDoc:original", "ShiftHome:original-return", "bubble:original-visible",
           "popup:original-open-focus", "compositionEnter:original-no-link", "Cancel:original-native-text",
           "Apply:original-selected-text", "save:original", "finally"]
boundary = [dict(copy.deepcopy(frame), stage=stage) for stage in actions]
transition = {"at": 20, "stage": "provider:status", "previousOwner": frame["owner"],
              "owner": "[11,12,11,13,14,15,16]", "bindingGeneration": 2}
data = {"frames": [frame], "critical": boundary, "actionBoundaries": boundary[:-1],
        "ownerChanges": [transition], "observedMismatches": [frame], "firstObservedState": frame,
        "updates": 1, "localUpdates": 1, "earlyBindingNotCaptured": True,
        "totals": {"frames": 1, "critical": 9, "actionBoundaries": 8, "ownerChanges": 1, "observedMismatches": 1},
        "dropped": {"frames": 0, "critical": 0, "actionBoundaries": 0, "ownerChanges": 0, "observedMismatches": 0}}
reference = {"name": name, "contentType": "application/json", "file": member}
network = {"type": "resource-snapshot", "snapshot": {"request": {"method": "GET", "url": "http://localhost/assets/fixture.js"},
           "response": {"status": 200}, "_resourceType": "script", "time": 1, "_monotonicTime": 1}}
def run(label, payload=data, ref=reference, test=None, extra=None, duplicate=False):
    archive = work / (label + ".zip")
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as z:
        z.writestr("0-trace.network", json.dumps(network))
        z.writestr("test.trace", test if test is not None else json.dumps({"type": "after", "attachments": [ref]}))
        z.writestr(member, payload if isinstance(payload, bytes) else json.dumps(payload))
        if extra:
            for path, value in extra: z.writestr(path, value)
        if duplicate: z.writestr(member, json.dumps(data))
    output = subprocess.check_output([sys.executable, script, str(archive)], text=True, timeout=20)
    assert "GET 200 script /assets/fixture.js" in output, label + ": base digest lost"
    for forbidden in [secret, "NATIVE_ID_PRIVATE", "private.example", "한글과", "😀", "rawFuture", "activeLabel", "pmDocument", "password"]:
        assert forbidden not in output, label + ": private field leaked"
    prefix = "w3-template-diagnostic "
    records = [json.loads(line[len(prefix):]) for line in output.splitlines() if line.startswith(prefix)]
    assert len(records) == 1, label + ": missing diagnostic outcome"
    return records[0]

valid = run("valid")
assert valid["available"] is True
assert valid["counts"]["frames"] == {"retained": 1, "total": 1, "dropped": 0, "unknown": False}
selected = valid["actions"]["ShiftHome:original-return"]
assert selected["native"] == {"inside": True, "codePoints": 8, "equalsKnownFixtureCjkEmoji": True,
                               "positions": {"anchor": 9, "head": 1}, "mappingUnknown": False}
assert selected["pm"] == {"anchor": 9, "head": 1, "empty": False, "type": "text"}
assert selected["auth"] == {"authenticated": True, "synced": True, "scope": "read-write", "status": "connected"}
assert selected["nativeId"]["present"] is True and len(selected["nativeId"]["hash"]) == 16
assert valid["firstIdentityChange"]["previousOwner"] == [1,2,1,3,4,5,6]
assert valid["firstIdentityChange"]["owner"] == [11,12,11,13,14,15,16]
assert valid["firstMismatch"]["native"]["positions"] == {"anchor": 9, "head": 1}
assert valid["missingActions"] == [] and valid["earlyBindingNotCaptured"] is True

hostile = copy.deepcopy(data)
hostile["frames"][0]["stage"] = "keydown:" + secret
hostile["frames"][0]["auth"]["scope"] = secret
hostile["frames"][0]["focus"]["activeTag"] = secret
hostile["frames"][0]["native"]["positions"] = {"unknown": secret}
hostile["frames"][0]["native"]["text"] = secret
safe = run("hostile", hostile)["tail"][0]
assert safe["stage"] == "unknown" and safe["auth"]["scope"] == "unknown"
assert safe["native"]["equalsKnownFixtureCjkEmoji"] is False and safe["native"]["mappingUnknown"] is True
assert safe["at"] == 10

truncated = copy.deepcopy(data)
truncated["totals"]["frames"] = 501
truncated["dropped"]["frames"] = 500
assert run("truncated", truncated)["counts"]["frames"]["dropped"] == 500
legacy = copy.deepcopy(data)
del legacy["totals"]; del legacy["dropped"]
assert run("legacy_unknown", legacy)["counts"]["frames"]["unknown"] is True

rejects = [
    ("path", dict(ref=dict(reference, file="../" + secret)), "invalid_attachment_reference"),
    ("content_type", dict(ref=dict(reference, contentType="text/plain")), "invalid_attachment_reference"),
    ("missing", dict(ref=dict(reference, file="attachments/" + "b" * 40)), "missing_or_invalid_attachment_member"),
    ("malformed", dict(payload=("{" + secret).encode()), "invalid_attachment_data"),
    ("oversize", dict(payload=b" " * (1024*1024 + 1)), "attachment_size_limit"),
    ("schema", dict(payload={"frames": secret}), "invalid_event_schema_or_limit"),
    ("duplicate", dict(duplicate=True), "missing_or_invalid_attachment_member"),
    ("events", dict(test="\n".join("{}" for _ in range(10001))), "test_event_limit"),
    ("bad_test", dict(test="{" + secret), "invalid_attachment_data"),
    ("non_json_number", dict(payload=json.dumps(data).replace('"at": 10', '"at": NaN').encode()), "invalid_attachment_data"),
    ("no_reference", dict(test="{}"), "missing_or_duplicate_attachment"),
    ("member_limit", dict(extra=[("unrelated/" + str(i), "") for i in range(4096)]), "member_limit"),
]
inconsistent = copy.deepcopy(data); inconsistent["totals"]["frames"] = 2
rejects.append(("counts", dict(payload=inconsistent), "inconsistent_collection_counts"))
malformed_snapshot = copy.deepcopy(data); malformed_snapshot["frames"][0]["native"] = secret
rejects.append(("nested_schema", dict(payload=malformed_snapshot), "invalid_snapshot_schema"))
for label, arguments, reason in rejects:
    assert run(label, **arguments) == {"available": False, "reason": reason}, label
print("trace-summary diagnostic fixtures: 4 valid/hostile/partial/legacy + 14 rejection controls PASS")
PY

# Pure observer contract fixture: real Y.Doc updates and test-owned public
# Editor/provider emitters. This tests callback ownership, not browser behavior.
cat >"$WORK/observer-fixture.cjs" <<'JS'
const assert = require("node:assert/strict");
const fs = require("node:fs");
const vm = require("node:vm");
const root = process.argv[2];
const ts = require(root + "/node_modules/typescript");
const Y = require(root + "/node_modules/yjs");
const text = fs.readFileSync(root + "/apps/web/e2e/editor-template-chrome-flow.spec.ts", "utf8");
const source = ts.createSourceFile("observer.ts", text, ts.ScriptTarget.Latest, true);
const fn = source.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === "observeTemplateSelection");
let callback;
function visit(node) {
  if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression) && node.expression.name.text === "addInitScript") callback = node.arguments[0];
  ts.forEachChild(node, visit);
}
assert.ok(fn); visit(fn); assert.ok(callback);
const js = ts.transpileModule("(" + callback.getText(source) + ")()", { compilerOptions: { target: ts.ScriptTarget.ES2023 } }).outputText;
class Events {
  listeners = new Map();
  on(name, callback) { const set = this.listeners.get(name) ?? new Set(); set.add(callback); this.listeners.set(name, set); }
  off(name, callback) { this.listeners.get(name)?.delete(callback); }
  emit(name, value) { for (const callback of [...(this.listeners.get(name) ?? [])]) callback(value); }
  count() { return [...this.listeners.values()].reduce((n, set) => n + set.size, 0); }
}
const docA = new Y.Doc(), docB = new Y.Doc();
const provider = (authenticated, scope, status) => Object.assign(new Events(), {
  isAuthenticated: authenticated, authorizedScope: scope, synced: authenticated,
  configuration: { websocketProvider: { status } },
});
const providerA = provider(true, "read-write", "connected"), providerB = provider(false, "readonly", "connecting");
const selection = { anchor: 9, head: 1, empty: false, toJSON: () => ({ type: "text" }) };
function editor(doc, provider) {
  const dom = { contentEditable: "true", contains: () => true };
  const state = { selection, storedMarks: null, doc: { toJSON: () => ({ type: "doc" }) } };
  const current = Object.assign(new Events(), {
    extensionManager: { extensions: [{ name: "collaboration", options: { document: doc } }, { name: "collaborationCaret", options: { provider } }] },
    view: { dom, state, hasFocus: () => true, posAtDOM: (_, offset) => offset, composing: false },
    state, isEditable: true, isDestroyed: false,
  });
  dom.editor = current; return dom;
}
const rootA = editor(docA, providerA), rootB = editor(docB, providerB);
let mounted = rootA, clock = 0, raf, nativeHead = 1;
const document = Object.assign(new Events(), {
  querySelector: selector => selector === ".fvoci-editor .ProseMirror" ? mounted : null,
  activeElement: { tagName: "DIV", getAttribute: () => null },
  addEventListener(name, callback) { this.on(name, callback); },
  removeEventListener(name, callback) { this.off(name, callback); },
});
const window = Object.assign(new Events(), {
  addEventListener(name, callback) { this.on(name, callback); },
  removeEventListener(name, callback) { this.off(name, callback); },
  getSelection: () => ({ anchorNode: {}, focusNode: {}, anchorOffset: 9, focusOffset: nativeHead, toString: () => "한글과 😀 링크" }) });
vm.runInNewContext(js, { document, window, performance: { now: () => ++clock }, KeyboardEvent: class {},
  requestAnimationFrame: callback => { raf = callback; return 1; }, cancelAnimationFrame: () => { raf = undefined; }, queueMicrotask: callback => callback() });
const observer = window.__w3TemplateObserver;
observer.checkpoint("openDoc:original");
docA.getMap("fixture").set("one", 1);
const retiredAuthenticated = [...providerA.listeners.get("authenticated")][0];
mounted = rootB;
observer.checkpoint("ShiftHome:original-return");
assert.equal(rootA.editor.count(), 0); assert.equal(providerA.count(), 0);
retiredAuthenticated();
docA.getMap("fixture").set("retired", 2);
docB.getMap("fixture").set("one", 1);
mounted = undefined;
observer.checkpoint("bubble:original-visible");
assert.equal(rootB.editor.count(), 0); assert.equal(providerB.count(), 0);
docB.getMap("fixture").set("during-gap", 2);
mounted = rootB;
observer.checkpoint("popup:original-open-focus");
for (let i = 0; i < 600; i++) {
  nativeHead = 2 + (i % 5);
  rootB.editor.emit("transaction", { transaction: { docChanged: true, selectionSet: false } });
}
rootB.editor.isDestroyed = true;
observer.checkpoint("Cancel:original-native-text");
const result = observer.stop();
const checkpoint = stage => result.actionBoundaries.find(frame => frame.stage === stage);
const first = checkpoint("openDoc:original"), second = checkpoint("ShiftHome:original-return");
assert.equal(first.clientID, docA.clientID);
assert.equal(second.clientID, docB.clientID);
assert.equal(second.auth.authenticated, false); assert.equal(second.auth.scope, "readonly");
assert.equal(second.auth.status, "connecting"); assert.equal(second.generationUpdates, 0);
assert.equal(second.bindingGeneration, 2);
const retired = result.firstRetiredEvent;
assert.equal(retired.eventBindingGeneration, 1); assert.equal(retired.bindingGeneration, 2);
assert.equal(retired.clientID, docB.clientID); assert.equal(retired.auth.authenticated, false);
assert.equal(checkpoint("bubble:original-visible").unavailable, "missing-editor");
assert.equal(checkpoint("popup:original-open-focus").bindingGeneration, 4);
assert.equal(checkpoint("popup:original-open-focus").generationUpdates, 0);
assert.equal(checkpoint("Cancel:original-native-text").unavailable, "destroyed-editor");
assert.equal(result.updates, 2); assert.equal(result.localUpdates, 2);
assert.equal(result.ownerChanges.length, 4); // A→B→gap→B→destroyed, distinct transitions.
assert.equal(result.totals.ownerChanges, 4);
assert.equal(result.firstObservedState.clientID, docA.clientID);
assert.equal(result.critical[0].stage, "openDoc:original");
assert.equal(result.observedMismatches[0].nativePositions.head, 2);
assert.equal(result.observedMismatches.at(-1).nativePositions.head, 6);
assert.equal(result.observedMismatches.length, 256);
assert.equal(result.totals.observedMismatches, 600);
assert.equal(result.dropped.observedMismatches, 344);
assert.equal(result.frames.length, 512); assert.equal(result.critical.length, 256);
assert.equal(result.totals.frames, result.frames.length + result.dropped.frames);
assert.equal(result.totals.critical, result.critical.length + result.dropped.critical);
assert.equal(rootB.editor.count(), 0); assert.equal(providerB.count(), 0); assert.equal(document.count(), 0); assert.equal(window.count(), 0);
assert.equal(raf, undefined);
const stoppedSnapshot = JSON.stringify(result);
retiredAuthenticated();
assert.equal(JSON.stringify(result), stoppedSnapshot);
assert.equal(rootA.editor.count(), 0); assert.equal(providerA.count(), 0);
assert.equal(rootB.editor.count(), 0); assert.equal(providerB.count(), 0);
docA.destroy(); docB.destroy();
console.log("template observer pure fixture: remount/current-owner, gap/destroyed, scoped counters and cleanup PASS");
JS

bun "$WORK/observer-fixture.cjs" "$ROOT"
