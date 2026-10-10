#!/usr/bin/env bash
# Test-only: the template observer's pure contract fixture, then its digest by
# tools/web-e2e/trace-summary.ts. The summary's redaction and diagnostic
# controls are Bun tests (tools/web-e2e/trace-summary.test.ts).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
WORK="$(mktemp -d "${TMPDIR:-/tmp}/fvoci-trace-summary.XXXXXX")"
trap 'rm -rf "$WORK"' EXIT

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
const zipPath = work + "/observer-gap.zip";
const { zipSync, strToU8 } = require(root + "/node_modules/fflate");
const member = "attachments/" + "c".repeat(40);
fs.writeFileSync(zipPath, zipSync({
  "test.trace": strToU8(JSON.stringify({ type: "after", attachments: [{ name: "w3-template-native-selection-observation.json", contentType: "application/json", file: member }] })),
  [member]: strToU8(JSON.stringify(result)),
}));
const summary = child.execFileSync(process.execPath, [root + "/tools/web-e2e/trace-summary.ts", zipPath], { encoding: "utf8" });
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
