import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import * as Y from "yjs";
import { z } from "zod";

type Callback = (value?: unknown) => void;
class Events {
  listeners = new Map<string, Set<Callback>>();
  on(name: string, callback: Callback) {
    const set = this.listeners.get(name) ?? new Set<Callback>();
    set.add(callback);
    this.listeners.set(name, set);
  }
  off(name: string, callback: Callback) {
    this.listeners.get(name)?.delete(callback);
  }
  emit(name: string, value?: unknown) {
    for (const callback of [...(this.listeners.get(name) ?? [])]) callback(value);
  }
  count() {
    return [...this.listeners.values()].reduce((n, set) => n + set.size, 0);
  }
}
interface Observer {
  checkpoint(stage: string): void;
  stop(): unknown;
}
const frameSchema = z.looseObject({
  stage: z.string(),
  clientID: z.number().optional(),
  bindingGeneration: z.number().optional(),
  eventBindingGeneration: z.number().optional(),
  generationUpdates: z.number().optional(),
  retiredEvent: z.boolean().optional(),
  unavailable: z.string().optional(),
  pm: z.unknown().optional(),
  auth: z
    .object({
      authenticated: z.boolean().optional(),
      scope: z.string().optional(),
      status: z.string().optional(),
    })
    .optional(),
  nativePositions: z.object({ head: z.number().optional() }).optional(),
});
const resultSchema = z.looseObject({
  actionBoundaries: z.array(frameSchema),
  critical: z.array(frameSchema),
  frames: z.array(frameSchema),
  firstRetiredEvent: frameSchema,
  firstObservedState: frameSchema,
  observedMismatches: z.array(frameSchema),
  updates: z.number(),
  localUpdates: z.number(),
  ownerChanges: z.array(frameSchema),
  totals: z.record(z.string(), z.number()),
  dropped: z.record(z.string(), z.number()),
});

/** A-only fixture: execute the current source callback; B must export test support before deleting the shell. */
export function observerFixture() {
  const text = readFileSync(
    resolve(import.meta.dir, "../../apps/web/e2e/editor-template-chrome-flow.spec.ts"),
    "utf8",
  );
  const source = ts.createSourceFile("observer.ts", text, ts.ScriptTarget.Latest, true);
  const fn = source.statements.find(
    (node) => ts.isFunctionDeclaration(node) && node.name?.text === "observeTemplateSelection",
  );
  assert.ok(fn, "Observer source must exist");
  let callback: ts.Node | undefined;
  function visit(node: ts.Node) {
    if (
      ts.isCallExpression(node) &&
      ts.isPropertyAccessExpression(node.expression) &&
      node.expression.name.text === "addInitScript"
    )
      callback = node.arguments[0];
    ts.forEachChild(node, visit);
  }
  visit(fn);
  assert.ok(callback, "Observer callback must exist");
  const js = ts.transpileModule("(" + callback.getText(source) + ")()", {
    compilerOptions: { target: ts.ScriptTarget.ES2023 },
  }).outputText;
  const docA = new Y.Doc(),
    docB = new Y.Doc();
  try {
    const provider = (authenticated: boolean, scope: string, status: string) =>
      Object.assign(new Events(), {
        isAuthenticated: authenticated,
        authorizedScope: scope,
        synced: authenticated,
        configuration: { websocketProvider: { status } },
      });
    const providerA = provider(true, "read-write", "connected"),
      providerB = provider(false, "readonly", "connecting");
    const selection = { anchor: 9, head: 1, empty: false, toJSON: () => ({ type: "text" }) };
    function editor(doc: Y.Doc, currentProvider: ReturnType<typeof provider>) {
      const dom = { contentEditable: "true", contains: () => true, editor: undefined as unknown };
      const state = { selection, storedMarks: null, doc: { toJSON: () => ({ type: "doc" }) } };
      const current = Object.assign(new Events(), {
        extensionManager: {
          extensions: [
            { name: "collaboration", options: { document: doc } },
            { name: "collaborationCaret", options: { provider: currentProvider } },
          ],
        },
        view: {
          dom,
          state,
          hasFocus: () => true,
          posAtDOM: (_node: unknown, offset: number) => offset,
          composing: false,
        },
        state,
        isEditable: true,
        isDestroyed: false,
      });
      dom.editor = current;
      return { dom, editor: current };
    }
    const rootA = editor(docA, providerA),
      rootB = editor(docB, providerB);
    let mounted: typeof rootA | undefined = rootA,
      clock = 0,
      raf: Callback | undefined,
      nativeHead = 1;
    const document = Object.assign(new Events(), {
      querySelector: (selector: string) =>
        selector === ".fvoci-editor .ProseMirror" ? mounted?.dom : null,
      activeElement: { tagName: "DIV", getAttribute: () => null },
      addEventListener(this: Events, name: string, cb: Callback) {
        this.on(name, cb);
      },
      removeEventListener(this: Events, name: string, cb: Callback) {
        this.off(name, cb);
      },
    });
    const window = Object.assign(new Events(), {
      __w3TemplateObserver: undefined as Observer | undefined,
      addEventListener(this: Events, name: string, cb: Callback) {
        this.on(name, cb);
      },
      removeEventListener(this: Events, name: string, cb: Callback) {
        this.off(name, cb);
      },
      getSelection: () => ({
        anchorNode: {},
        focusNode: {},
        anchorOffset: 9,
        focusOffset: nativeHead,
        toString: () => "한글과 😀 링크",
      }),
    });
    runInNewContext(js, {
      document,
      window,
      performance: { now: () => ++clock },
      KeyboardEvent: Event,
      requestAnimationFrame: (cb: Callback) => {
        raf = cb;
        return 1;
      },
      cancelAnimationFrame: () => {
        raf = undefined;
      },
      queueMicrotask: (cb: Callback) => {
        cb();
      },
    });
    const observer = window.__w3TemplateObserver;
    assert.ok(observer);
    observer.checkpoint("openDoc:original");
    docA.getMap("fixture").set("one", 1);
    const retiredAuthenticated = [...(providerA.listeners.get("authenticated") ?? [])][0];
    assert.ok(retiredAuthenticated);
    mounted = rootB;
    observer.checkpoint("ShiftHome:original-return");
    assert.equal(rootA.editor.count(), 0);
    assert.equal(providerA.count(), 0);
    docA.getMap("fixture").set("retired", 2);
    docB.getMap("fixture").set("one", 1);
    mounted = undefined;
    observer.checkpoint("bubble:original-visible");
    retiredAuthenticated();
    assert.equal(rootB.editor.count(), 0);
    assert.equal(providerB.count(), 0);
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
    const raw = observer.stop(),
      result = resultSchema.parse(raw);
    const checkpoint = (stage: string) => {
      const frame = result.actionBoundaries.find((value) => value.stage === stage);
      assert.ok(frame);
      return frame;
    };
    const first = checkpoint("openDoc:original"),
      second = checkpoint("ShiftHome:original-return");
    assert.equal(first.clientID, docA.clientID);
    assert.equal(second.clientID, docB.clientID);
    assert.ok(second.auth);
    assert.equal(second.auth.authenticated, false);
    assert.equal(second.auth.scope, "readonly");
    assert.equal(second.auth.status, "connecting");
    assert.equal(second.generationUpdates, 0);
    assert.equal(second.bindingGeneration, 2);
    const retired = result.firstRetiredEvent;
    assert.equal(retired.eventBindingGeneration, 1);
    assert.equal(retired.bindingGeneration, 3);
    assert.equal(retired.unavailable, "missing-editor");
    assert.equal(retired.retiredEvent, true);
    assert.equal(retired.clientID, undefined);
    assert.equal(retired.auth, undefined);
    assert.equal(retired.pm, undefined);
    const destroyed = result.critical.find(
      (value) => value.retiredEvent && value.unavailable === "destroyed-editor",
    );
    assert.ok(destroyed);
    assert.equal(destroyed.eventBindingGeneration, 1);
    assert.equal(destroyed.bindingGeneration, 5);
    assert.equal(destroyed.auth, undefined);
    assert.equal(checkpoint("bubble:original-visible").unavailable, "missing-editor");
    assert.equal(checkpoint("popup:original-open-focus").bindingGeneration, 4);
    assert.equal(checkpoint("popup:original-open-focus").generationUpdates, 0);
    assert.equal(checkpoint("Cancel:original-native-text").unavailable, "destroyed-editor");
    assert.equal(result.updates, 2);
    assert.equal(result.localUpdates, 2);
    assert.equal(result.ownerChanges.length, 4);
    assert.equal(result.totals.ownerChanges, 4);
    assert.equal(result.firstObservedState.clientID, docA.clientID);
    assert.equal(result.critical[0]?.stage, "openDoc:original");
    assert.equal(result.observedMismatches[0]?.nativePositions?.head, 2);
    assert.equal(result.observedMismatches.at(-1)?.nativePositions?.head, 6);
    assert.equal(result.observedMismatches.length, 256);
    assert.equal(result.totals.observedMismatches, 600);
    assert.equal(result.dropped.observedMismatches, 344);
    assert.equal(result.frames.length, 512);
    assert.equal(result.critical.length, 256);
    assert.equal(result.totals.frames, result.frames.length + (result.dropped.frames ?? NaN));
    assert.equal(result.totals.critical, result.critical.length + (result.dropped.critical ?? NaN));
    for (const emitter of [rootA.editor, rootB.editor, providerA, providerB, document, window])
      assert.equal(emitter.count(), 0);
    assert.equal(raf, undefined);
    const stopped = JSON.stringify(raw);
    retiredAuthenticated();
    assert.equal(JSON.stringify(raw), stopped);
    for (const emitter of [rootA.editor, rootB.editor, providerA, providerB])
      assert.equal(emitter.count(), 0);
    return raw;
  } finally {
    docA.destroy();
    docB.destroy();
  }
}
