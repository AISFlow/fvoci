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
    assert len(json.dumps(records[0]).encode()) <= 49152, label + ": diagnostic output bound lost"
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
assert valid["missingActions"] == ["caret:after-click", "caret:after-End"]
assert all(valid["actions"][action] is not None for action in actions)
assert valid["earlyBindingNotCaptured"] is True

caret_data = copy.deepcopy(data)
caret_data["caretBoundaries"] = [
    {"stage": "after-click", "ownerRecordStage": "caret:after-click", "at": 5,
     "native": {"anchor": {"inside": True, "noneditableLeaf": False, "position": 5},
                "head": {"inside": True, "noneditableLeaf": False, "position": 5}, "text": secret},
     "wide": True, "rich": True, "rawDom": secret},
    {"stage": "after-End", "ownerRecordStage": "caret:after-End", "at": 7,
     "native": {"anchor": {"inside": False, "noneditableLeaf": True, "position": 9, "nodeAttrs": secret},
                "head": {"inside": True, "noneditableLeaf": False, "position": 9}, "text": secret},
     "wide": False, "rich": False, "focus": {"activeLabel": secret}, "error": secret},
]
for label, at, pos in (("caret:after-click", 5, 5), ("caret:after-End", 7, 9)):
    captured = dict(copy.deepcopy(frame), stage=label, at=at,
                    pm={"anchor": pos, "head": pos, "empty": True, "type": "text"})
    captured["native"]["positions"] = {"anchor": pos, "head": pos}
    caret_data["critical"].append(captured)
    caret_data["actionBoundaries"].append(captured)
caret_data["totals"]["critical"] += 2
caret_data["totals"]["actionBoundaries"] += 2
caret_valid = run("caret_valid", caret_data)
assert caret_valid["missingActions"] == []
click = caret_valid["actions"]["caret:after-click"]
end = caret_valid["actions"]["caret:after-End"]
assert click["caret"] == {"anchor": {"inside": True, "noneditableLeaf": False, "position": 5},
                          "head": {"inside": True, "noneditableLeaf": False, "position": 5},
                          "wide": True, "rich": True}
assert end["caret"] == {"anchor": {"inside": False, "noneditableLeaf": True, "position": 9},
                        "head": {"inside": True, "noneditableLeaf": False, "position": 9},
                        "wide": False, "rich": False}
assert click["owner"] == [1,2,1,3,4,5,6] and end["owner"] == [1,2,1,3,4,5,6]
assert click["pm"] == {"anchor": 5, "head": 5, "empty": True, "type": "text"}
assert end["native"]["positions"] == {"anchor": 9, "head": 9}
assert end["focus"]["editor"] is True
assert valid["actions"]["ShiftHome:original-return"]["caret"] is None
caret_unknown = copy.deepcopy(caret_data)
caret_unknown["caretBoundaries"][1] = {
    "stage": "after-End", "ownerRecordStage": "caret:after-End", "at": 7,
    "native": {"anchor": {"inside": None, "noneditableLeaf": None, "position": None}, "head": {}},
}
assert run("caret_unknown", caret_unknown)["actions"]["caret:after-End"]["caret"] == {
    "anchor": {"inside": None, "noneditableLeaf": None, "position": None},
    "head": {"inside": None, "noneditableLeaf": None, "position": None}, "wide": None, "rich": None,
}
caret_rejects = []
for label, value in (("array", secret), ("overflow", caret_data["caretBoundaries"] * 2),
                     ("duplicate", [caret_data["caretBoundaries"][0]] * 2),
                     ("entry", [secret])):
    invalid = copy.deepcopy(caret_data); invalid["caretBoundaries"] = value
    caret_rejects.append((label, invalid, "invalid_caret_boundary_schema"))
for label, key, value, reason in (
    ("stage", "stage", secret, "invalid_caret_boundary_schema"),
    ("owner_stage", "ownerRecordStage", "caret:after-click", "invalid_caret_boundary_schema"),
    ("at", "at", secret, "invalid_caret_boundary_schema"),
    ("native", "native", secret, "invalid_caret_boundary_schema"),
    ("wide", "wide", "true", "invalid_boolean_schema"),
    ("rich", "rich", 1, "invalid_boolean_schema"),
):
    invalid = copy.deepcopy(caret_data); invalid["caretBoundaries"][1][key] = value
    caret_rejects.append((label, invalid, reason))
for label, key, value, reason in (
    ("endpoint", None, secret, "invalid_caret_boundary_schema"),
    ("inside", "inside", 1, "invalid_boolean_schema"),
    ("leaf", "noneditableLeaf", "false", "invalid_boolean_schema"),
    ("position_type", "position", "9", "invalid_position_schema"),
    ("position_range", "position", 1_000_000_001, "invalid_position_schema"),
):
    invalid = copy.deepcopy(caret_data)
    if key is None: invalid["caretBoundaries"][1]["native"]["anchor"] = value
    else: invalid["caretBoundaries"][1]["native"]["anchor"][key] = value
    caret_rejects.append((label, invalid, reason))
for label, payload, reason in caret_rejects:
    assert run("caret_reject_" + label, payload) == {"available": False, "reason": reason}, label
print("trace-summary caret fixtures: 2 exact/private/nullable inputs + 15 typed/shape/overflow rejection controls PASS")

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
const work = process.argv[3];
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
docA.getMap("fixture").set("retired", 2);
docB.getMap("fixture").set("one", 1);
mounted = undefined;
observer.checkpoint("bubble:original-visible");
retiredAuthenticated();
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
retiredAuthenticated();
const result = observer.stop();
const checkpoint = stage => result.actionBoundaries.find(frame => frame.stage === stage);
const first = checkpoint("openDoc:original"), second = checkpoint("ShiftHome:original-return");
assert.equal(first.clientID, docA.clientID);
assert.equal(second.clientID, docB.clientID);
assert.equal(second.auth.authenticated, false); assert.equal(second.auth.scope, "readonly");
assert.equal(second.auth.status, "connecting"); assert.equal(second.generationUpdates, 0);
assert.equal(second.bindingGeneration, 2);
const retired = result.firstRetiredEvent;
assert.ok(retired, "First retired callback during missing-editor gap must survive overflow");
assert.equal(retired.eventBindingGeneration, 1); assert.equal(retired.bindingGeneration, 3);
assert.equal(retired.unavailable, "missing-editor"); assert.equal(retired.retiredEvent, true);
assert.equal(retired.clientID, undefined); assert.equal(retired.auth, undefined); assert.equal(retired.pm, undefined);
const destroyedRetired = result.critical.find(frame => frame.retiredEvent && frame.unavailable === "destroyed-editor");
assert.ok(destroyedRetired); assert.equal(destroyedRetired.eventBindingGeneration, 1);
assert.equal(destroyedRetired.bindingGeneration, 5); assert.equal(destroyedRetired.auth, undefined);
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
const child = require("node:child_process");
const jsonPath = work + "/observer-gap.json", zipPath = work + "/observer-gap.zip";
fs.writeFileSync(jsonPath, JSON.stringify(result));
child.execFileSync("python3", ["-c", `
import json,sys,zipfile
member="attachments/"+"c"*40
with zipfile.ZipFile(sys.argv[2],"w") as archive:
 archive.writestr("test.trace",json.dumps({"type":"after","attachments":[{"name":"w3-template-native-selection-observation.json","contentType":"application/json","file":member}]}))
 archive.writestr(member,open(sys.argv[1],"rb").read())
`, jsonPath, zipPath]);
const summary = child.execFileSync("python3", [root + "/scripts/web-e2e-trace-summary.py", zipPath], { encoding: "utf8" });
const prefix = "w3-template-diagnostic ";
const digest = JSON.parse(summary.split("\n").find(line => line.startsWith(prefix)).slice(prefix.length));
assert.equal(digest.available, true);
assert.equal(digest.firstRetiredEvent.unavailable, "missing-editor");
assert.equal(digest.firstRetiredEvent.eventBindingGeneration, 1);
assert.equal(digest.firstRetiredEvent.bindingGeneration, 3);
assert.equal(digest.firstRetiredEvent.retiredEvent, true);
assert.deepEqual(digest.firstRetiredEvent.auth, { authenticated: null, synced: null, scope: "unknown", status: "unknown" });
assert.deepEqual(digest.firstRetiredEvent.owner, [null,null,null,null,null,null,null]);
assert.equal(digest.counts.critical.unknown, false);
assert.ok(digest.counts.critical.dropped > 256);
assert.ok(!summary.includes("한글과") && !summary.includes("😀") && !summary.includes("pmDocument"));
docA.destroy(); docB.destroy();
console.log("template observer pure fixture: remount/current-owner, gap/destroyed, scoped counters and cleanup PASS");
JS

bun "$WORK/observer-fixture.cjs" "$ROOT" "$WORK"

# Task observer packets use a separate schema. Only the safe digest is exported
# through the existing browser-summary; raw trace/attachment uploads stay excluded.
cat >"$WORK/task-producer-fixture.cjs" <<'JS'
const assert = require("node:assert/strict");
const fs = require("node:fs"), vm = require("node:vm");
const root = process.argv[2], work = process.argv[3];
const ts = require(root + "/node_modules/typescript");
const text = fs.readFileSync(root + "/apps/web/e2e/main-alignment-task-contract.spec.ts", "utf8");
const source = ts.createSourceFile("task-observer.ts", text, ts.ScriptTarget.Latest, true);
const names = ["taskAdmissionFields", "taskAdmissionPacket", "attachTaskAdmissionDiagnostic"];
const functions = names.map(name => source.statements.find(n => ts.isFunctionDeclaration(n) && n.name?.text === name));
const kinds = source.statements.find(n => ts.isVariableStatement(n) && n.declarationList.declarations.some(d => d.name.getText(source) === "taskAdmissionEventKinds"));
assert.ok(kinds && functions.every(Boolean));
const js = ts.transpileModule([kinds, ...functions].map(n => n.getText(source)).join("\n"),
  { compilerOptions: { target: ts.ScriptTarget.ES2023 } }).outputText;
const context = vm.createContext({ setTimeout, clearTimeout });
vm.runInContext(js, context);
(async () => {
  const captured = [];
  await context.attachTaskAdmissionDiagnostic({ attach: async (name, options) => captured.push({ name, ...options }) },
    "retired-grant", async () => ({ route: { events: [{ kind: "auth-received", order: 3, socket: 2, scope: "readonly", token: "PRODUCER_PRIVATE_TOKEN" }], dropped: 0 }, completed: false }));
  assert.equal(captured.length, 1);
  assert.equal(captured[0].name, "w3-task-retired-grant-wire-coherence.json");
  assert.equal(captured[0].contentType, "application/json");
  assert.ok(!captured[0].body.includes("PRODUCER_PRIVATE_TOKEN"));
  fs.writeFileSync(work + "/actual-task-attachment.json", JSON.stringify(captured[0]));
})().catch(error => { console.error(error); process.exitCode = 1; });
JS
bun "$WORK/task-producer-fixture.cjs" "$ROOT" "$WORK"

python3 - "$ROOT/scripts/web-e2e-trace-summary.py" "$WORK" <<'PY'
import copy, json, pathlib, struct, subprocess, sys, zipfile
script, work = sys.argv[1], pathlib.Path(sys.argv[2])
member = "attachments/" + "d" * 40
secret = "TASK_PACKET_PRIVATE_VALUE"
data = {
    "schema": "w3-task-fresh-admission-v1", "routeDeliveryMeaning": "route-send-returned",
    "routeToProviderSocketBinding": "unknown", "crossLaneClockOrder": "unknown",
    "route": {"events": [
        {"kind": "transport-open", "order": 1, "elapsedMs": 0, "socket": 1},
        {"kind": "auth-received", "order": 2, "elapsedMs": 3, "socket": 1, "frame": 1, "scope": "readonly"},
        {"kind": "frame-hold", "order": 3, "elapsedMs": 4, "socket": 1, "frame": 1},
        {"kind": "frame-release", "order": 4, "elapsedMs": 6, "socket": 1, "frame": 1},
        {"kind": "frame-delivery", "order": 5, "elapsedMs": 6, "socket": 1, "frame": 1},
        {"kind": "cleanup-start", "order": 6, "elapsedMs": 9, "completed": False},
    ], "dropped": 0},
    "browser": {"events": [
        {"kind": "browser-open", "order": 1, "elapsedMs": 0, "socket": 1},
        {"kind": "browser-authenticated", "order": 2, "elapsedMs": 8,
         "authenticationEpoch": 2, "scope": "readonly", "sameSocket": True},
        {"kind": "browser-close", "order": 3, "elapsedMs": 9, "socket": 1, "code": 1000},
    ], "dropped": 0},
    "state": {"authenticated": True, "scope": "readonly", "editable": False,
              "dom": "false", "saveEnabled": False, "sameDoc": True},
    "completed": False,
}
network = {"type": "resource-snapshot", "snapshot": {
    "request": {"method": "GET", "url": "http://localhost/assets/task-fixture.js"},
    "response": {"status": 200}, "_resourceType": "script", "time": 1, "_monotonicTime": 1}}
checks = 0
def run(label, payload=data, schedule="retired-grant", ref=None, test=None, extra=(), mode=None, encrypt=False, unsupported=False,
        template_reason="missing_or_duplicate_attachment"):
    global checks
    archive = work / ("task-" + label + ".zip")
    reference = ref if ref is not None else {
        "name": "w3-task-" + schedule + "-wire-coherence.json",
        "contentType": "application/json", "file": member}
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as z:
        z.writestr("0-trace.network", json.dumps(network))
        z.writestr("test.trace", test if test is not None else json.dumps({"type": "after", "attachments": [reference]}))
        item = zipfile.ZipInfo(member)
        if mode is not None: item.external_attr = mode << 16
        z.writestr(item, payload if isinstance(payload, bytes) else json.dumps(payload))
        for name, value in extra: z.writestr(name, value)
    if encrypt or unsupported:
        # ZipFile clears encryption flags when writing: mark this fixture's
        # member in both local/central headers without ever invoking encryption.
        raw = bytearray(archive.read_bytes())
        with zipfile.ZipFile(archive) as z: offset = z.getinfo(member).header_offset
        if encrypt: struct.pack_into("<H", raw, offset + 6, struct.unpack_from("<H", raw, offset + 6)[0] | 1)
        if unsupported: struct.pack_into("<H", raw, offset + 8, 99)
        offset = 0
        while True:
            offset = raw.find(b"PK\x01\x02", offset)
            if offset < 0: raise AssertionError("fixture member central header absent")
            length = struct.unpack_from("<H", raw, offset + 28)[0]
            if raw[offset + 46:offset + 46 + length].decode() == member:
                if encrypt: struct.pack_into("<H", raw, offset + 8, struct.unpack_from("<H", raw, offset + 8)[0] | 1)
                if unsupported: struct.pack_into("<H", raw, offset + 10, 99)
                break
            offset += 46 + length
        archive.write_bytes(raw)
    output = subprocess.check_output([sys.executable, script, str(archive)], text=True, timeout=20)
    assert "GET 200 script /assets/task-fixture.js" in output, label + ": base digest lost"
    assert secret not in output, label + ": private value leaked"
    template = [x for x in output.splitlines() if x.startswith("w3-template-diagnostic ")]
    assert template == ["w3-template-diagnostic " + json.dumps(
        {"available": False, "reason": template_reason}, separators=(",", ":"))], label
    prefix = "w3-task-admission-diagnostic "
    records = [json.loads(x[len(prefix):]) for x in output.splitlines() if x.startswith(prefix)]
    assert len(records) == 1 and len(json.dumps(records[0]).encode()) <= 1024 * 1024, label
    checks += 1
    return records[0]

for schedule in ("natural", "server-readonly", "retired-grant"):
    result = run(schedule, schedule=schedule)
    assert result["available"] and result["schedule"] == schedule and result["completed"] is False
    assert [x["order"] for x in result["route"]["events"]] == [1, 2, 3, 4, 5, 6]
    assert [x["kind"] for x in result["route"]["events"]][2:5] == ["frame-hold", "frame-release", "frame-delivery"]
    assert result["browser"]["events"][1]["authenticationEpoch"] == 2
    assert result["state"]["editable"] is False and result["state"]["saveEnabled"] is False
    assert result["routeDeliveryMeaning"] == "route-send-returned"
    assert result["routeToProviderSocketBinding"] == result["crossLaneClockOrder"] == "unknown"

missing = copy.deepcopy(data); missing["browser"] = {"events": []}; missing["state"] = {}
result = run("missing-capture", missing)
assert result["available"] and result["browser"] == {"events": [], "dropped": "unknown"}
assert all(x == "unknown" for x in result["state"].values())
assert result["completed"] is False
full = copy.deepcopy(data); full["completed"] = True
fields = {**{key: 1_000_000 for key in (
    "socket", "activeSocket", "frame", "authenticationEpoch", "initialAuthenticationEpoch", "updates",
    "localUpdates", "unauthorizedLocalWrites", "violations", "dropped", "requestCount", "ackCount")},
    **{key: False for key in ("authenticated", "editable", "canPersistAffordance", "saveEnabled", "ownerBroken",
    "sameRoot", "sameEditor", "sameElement", "sameDoc", "sameProvider", "sameClientId", "sameSocket",
    "archived", "canEdit", "completed")}, "elapsedMs": 3_600_000, "code": 4999,
    "scope": "read-write", "authenticatedScope": "readonly", "status": "disconnected", "dom": "false"}
for lane in ("route", "browser"):
    full[lane] = {"events": [{**fields, "kind": "browser-state", "order": i} for i in [*range(1, 17), *range(100, 340)]], "dropped": 83}
result = run("full-bounded", full)
assert result["completed"] is True and len(result["route"]["events"]) == 256
assert result["route"]["dropped"] == 83 and result["route"]["events"][16]["order"] == 100
unknown = copy.deepcopy(data); unknown["browser"]["events"][0] = {"kind": "browser-open", "order": "unknown", "code": "unknown", "sameSocket": "unknown"}
assert run("unknown-fields", unknown)["browser"]["events"][0]["order"] == "unknown"
captured = json.loads((work / "actual-task-attachment.json").read_text())
connected = run("actual-producer", json.loads(captured["body"]), ref={
    "name": captured["name"], "contentType": captured["contentType"], "file": member})
assert connected["available"] and connected["completed"] is False
assert connected["route"]["events"][0]["kind"] == "auth-received"
assert connected["route"]["events"][0]["order"] == 3 and connected["route"]["events"][0]["socket"] == 2
assert connected["route"]["events"][0]["scope"] == "readonly"
assert connected["browser"] == {"events": [], "dropped": "unknown"}
assert all(x == "unknown" for x in connected["state"].values())

reference = {"name": "w3-task-retired-grant-wire-coherence.json", "contentType": "application/json", "file": member}
rejects = [
    ("wrong-name", {"schedule": secret}, "invalid_attachment_name"),
    ("wrong-type", {"ref": dict(reference, contentType="text/plain")}, "invalid_attachment_reference"),
    ("path", {"ref": dict(reference, file="../" + secret)}, "invalid_attachment_reference"),
    ("noncanonical", {"ref": dict(reference, file="attachments/" + "D" * 40)}, "invalid_attachment_reference"),
    ("reference-private", {"ref": dict(reference, raw=secret)}, "invalid_attachment_reference"),
    ("missing-member", {"ref": dict(reference, file="attachments/" + "e" * 40)}, "missing_or_invalid_attachment_member"),
    ("duplicate-ref", {"test": json.dumps({"attachments": [reference, reference]})}, "missing_or_duplicate_attachment"),
    ("no-ref", {"test": "{}"}, "missing_or_duplicate_attachment"),
    ("duplicate-member", {"extra": [(member, json.dumps(data))]}, "missing_or_invalid_attachment_member"),
    ("nonregular", {"mode": 0o120777}, "missing_or_invalid_attachment_member"),
    ("encrypted", {"encrypt": True}, "missing_or_invalid_attachment_member"),
    ("unsupported-compression", {"unsupported": True}, "invalid_attachment_data"),
    ("oversize-body", {"payload": b" " * (1024 * 1024 + 1)}, "attachment_size_limit"),
    ("malformed", {"payload": ("{" + secret).encode()}, "invalid_attachment_data"),
    ("nonfinite", {"payload": json.dumps(data).replace('"order": 1', '"order": NaN').encode()}, "invalid_attachment_data"),
    ("duplicate-key", {"payload": ('{"schema":"' + secret + '",' + json.dumps(data)[1:]).encode()}, "invalid_attachment_data"),
    ("test-limit", {"test": "\n".join("{}" for _ in range(10001)), "template_reason": "test_event_limit"}, "test_event_limit"),
    ("test-size", {"test": " " * (4 * 1024 * 1024 + 1), "template_reason": "test_trace_size_limit"}, "test_trace_size_limit"),
    ("member-limit", {"extra": [("unrelated/" + str(i), "") for i in range(4096)], "template_reason": "member_limit"}, "member_limit"),
    ("bad-test", {"test": "{" + secret, "template_reason": "invalid_attachment_data"}, "invalid_attachment_data"),
    ("attachment-list", {"test": json.dumps({"attachments": [reference] * 33}), "template_reason": "invalid_attachment_list"}, "invalid_attachment_list"),
    ("invalid-reference", {"test": json.dumps({"attachments": [secret]})}, "invalid_attachment_reference"),
]
for key in ("schema", "routeDeliveryMeaning", "routeToProviderSocketBinding", "crossLaneClockOrder", "completed"):
    invalid = copy.deepcopy(data); invalid[key] = secret
    rejects.append(("policy-" + key, {"payload": invalid}, "invalid_policy_or_completed"))
for key, value in (("private", secret), ("route", []), ("state", secret)):
    invalid = copy.deepcopy(data); invalid[key] = value
    rejects.append(("shape-" + key, {"payload": invalid}, "invalid_schema" if key == "private" else "invalid_fields_or_lane"))
for key, value in (("raw", secret), ("kind", secret), ("order", True), ("order", -1), ("order", 1_000_001),
                   ("elapsedMs", 3_600_001), ("code", 5000), ("editable", "false"), ("scope", secret),
                   ("status", secret), ("dom", "inherit"), ("sameSocket", 1), ("socket", 1.5)):
    invalid = copy.deepcopy(data); invalid["route"]["events"][0][key] = value
    rejects.append(("event-" + key + "-" + str(len(rejects)), {"payload": invalid}, "invalid_fields_or_lane"))
for label in ("lane-overflow", "lane-order", "lane-private", "dropped-type", "state-private"):
    invalid = copy.deepcopy(data)
    if label == "lane-overflow": invalid["route"]["events"] *= 43
    if label == "lane-order": invalid["route"]["events"][1]["order"] = 1
    if label == "lane-private": invalid["browser"]["raw"] = secret
    if label == "dropped-type": invalid["browser"]["dropped"] = True
    if label == "state-private": invalid["state"]["token"] = secret
    rejects.append((label, {"payload": invalid}, "invalid_fields_or_lane"))
for label, arguments, reason in rejects:
    assert run(label, **arguments) == {"available": False, "reason": reason}, label
print(f"trace-summary Task fixtures: {checks - len(rejects)} positive + {len(rejects)} rejection controls PASS")
PY

# Supported DEFLATE decoding can fail independently of ZIP headers or method99.
# Keep a genuine compressed-body regression, including stdout/stderr privacy.
python3 - "$ROOT/scripts/web-e2e-trace-summary.py" "$WORK" <<'PY'
import json, pathlib, struct, subprocess, sys, zipfile, zlib
script, work = sys.argv[1], pathlib.Path(sys.argv[2])
member = "attachments/" + "f" * 40
canary = "TASK_DEFLATE_PRIVATE_BODY"
data = {"schema": "w3-task-fresh-admission-v1", "routeDeliveryMeaning": "route-send-returned",
        "routeToProviderSocketBinding": "unknown", "crossLaneClockOrder": "unknown",
        "route": {"events": [], "dropped": 0}, "browser": {"events": []}, "state": {}, "completed": False}
reference = {"name": "w3-task-retired-grant-wire-coherence.json", "contentType": "application/json", "file": member}
for corrupt in (False, True):
    archive = work / ("task-supported-deflate-corrupt.zip" if corrupt else "task-supported-deflate-valid.zip")
    payload = dict(data, privateBody=canary) if corrupt else data
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as trace:
        trace.writestr("test.trace", json.dumps({"type": "after", "attachments": [reference]}))
        item = zipfile.ZipInfo(member); item.compress_type = zipfile.ZIP_DEFLATED; item.external_attr = 0o100600 << 16
        trace.writestr(item, json.dumps(payload))
    if corrupt:
        raw = bytearray(archive.read_bytes())
        with zipfile.ZipFile(archive) as trace:
            item = trace.getinfo(member)
            assert item.compress_type == 8 and item.flag_bits == 0 and not item.is_dir()
            assert item.external_attr >> 16 == 0o100600
            header = item.header_offset
        name_size, extra_size = struct.unpack_from("<HH", raw, header + 26)
        offset = header + 30 + name_size + extra_size
        assert json.loads(zlib.decompress(raw[offset:offset + item.compress_size], -15)) == payload
        raw[offset] = 0x07  # BFINAL1 + reserved BTYPE3: genuinely invalid DEFLATE block.
        try:
            zlib.decompress(raw[offset:offset + item.compress_size], -15)
            raise AssertionError("fixture corruption did not fail supported decoding")
        except zlib.error:
            pass
        archive.write_bytes(raw)
    result = subprocess.run([sys.executable, script, str(archive)], capture_output=True, timeout=20)
    assert result.returncode == 0 and result.stderr == b"", "supported-DEFLATE containment failed"
    assert canary.encode() not in result.stdout + result.stderr
    prefix = "w3-task-admission-diagnostic "
    records = [json.loads(line[len(prefix):]) for line in result.stdout.decode().splitlines() if line.startswith(prefix)]
    assert len(records) == 1
    if corrupt:
        assert records[0] == {"available": False, "reason": "invalid_attachment_data"}
        assert b"zlib.error" not in result.stdout and b"Traceback" not in result.stdout
    else:
        assert records[0]["available"] is True and records[0]["completed"] is False
print("trace-summary Task DEFLATE fixtures: 1 valid supported decoder + 1 corrupt-body fixed/private refusal PASS")
PY
