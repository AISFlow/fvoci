import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";
import ts from "typescript";
import * as Vue from "vue";
import { Editor } from "@tiptap/core";
import type { Node as PmNode } from "@tiptap/pm/model";
import { splitBlock } from "@tiptap/pm/commands";
import {
  EditorState,
  NodeSelection,
  TextSelection,
  type Plugin,
  type Transaction,
} from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import { initProseMirrorDoc, ySyncPlugin, ySyncPluginKey, yUndoPlugin } from "@tiptap/y-tiptap";
import * as Y from "yjs";
import { tiptapJsonToYDoc, yDocToTiptapJson } from "../src/collab-tiptap.ts";
import { rawEditorPreflight, SourceModeSession } from "../src/source-mode.ts";
import { createFvociExtensions } from "../src/tiptap-schema.ts";
import { mdToTiptapJson } from "../src/markdown/parse.ts";
import {
  applyEditorModePreviewStyles,
  attachmentPreviewSpec,
  embedPreviewSpec,
  editorModePreview,
  sanitizeEditorModePreview,
} from "../src/vue/editor-mode-preview.ts";
import type { EntitySnapshot } from "../src/entities.ts";

function previewAbortListenerWitness() {
  const controller = new AbortController();
  const listeners = new Set<EventListenerOrEventListenerObject>();
  const counts = { added: 0, removed: 0 };
  const add = controller.signal.addEventListener.bind(controller.signal);
  const remove = controller.signal.removeEventListener.bind(controller.signal);
  Object.defineProperty(controller.signal, "addEventListener", {
    value: (
      type: string,
      listener: EventListenerOrEventListenerObject | null,
      options?: AddEventListenerOptions | boolean,
    ) => {
      if (!listener) return;
      if (type === "abort") {
        counts.added++;
        listeners.add(listener);
      }
      add(type, listener, options);
    },
  });
  Object.defineProperty(controller.signal, "removeEventListener", {
    value: (
      type: string,
      listener: EventListenerOrEventListenerObject | null,
      options?: EventListenerOptions | boolean,
    ) => {
      if (!listener) return;
      if (type === "abort") {
        counts.removed++;
        listeners.delete(listener);
      }
      remove(type, listener, options);
    },
  });
  return { controller, listeners, counts };
}

await test("owned preview wait aborts before a pending host resolver settles, with no late serialization or Yjs change", async () => {
  const ydoc = tiptapJsonToYDoc({
    type: "doc",
    content: [
      { type: "embed", attrs: { id: "abort-preview-ref", entity: "document", ref: "target" } },
      {
        type: "paragraph",
        attrs: { id: "abort-preview-tail" },
        content: [{ type: "text", text: "한글 🧑‍💻" }],
      },
    ],
  });
  const live = liveEditor(ydoc);
  const before = Y.encodeStateAsUpdate(ydoc);
  let serializations = 0;
  Object.defineProperty(live.editor.view, "dom", {
    value: {
      ownerDocument: {
        createElement() {
          serializations++;
          throw new Error("An aborted producer must not serialize");
        },
      },
    },
  });
  let resolve: (value: EntitySnapshot | null) => void = () => {};
  const hostPending = new Promise<EntitySnapshot | null>((done) => {
    resolve = done;
  });
  const { controller, listeners, counts } = previewAbortListenerWitness();
  const reason = new Error("owned preview retired");
  const rendering = editorModePreview(
    live.editor,
    { entityResolver: () => hostPending },
    controller.signal,
  );
  let rejected = false;
  const rejection = rendering.catch((error: unknown) => {
    assert.equal(error, reason);
    rejected = true;
  });
  try {
    controller.abort(reason);
    assert.equal(listeners.size, 0, "owned abort subscription is removed immediately");
    for (let index = 0; index < 5; index++) await Promise.resolve();
    assert.equal(rejected, true, "owned wait retires before host-owned IO completes");
    assert.equal(serializations, 0);
    resolve({ label: "late target", icon: "", status: "late" });
    await rejection;
    for (let index = 0; index < 5; index++) await Promise.resolve();
    assert.equal(serializations, 0);
    assert.deepEqual(counts, { added: 1, removed: 1 });
    assert.deepEqual(Y.encodeStateAsUpdate(ydoc), before);
    assert.equal(live.manager.undoStack.length, 0);
  } finally {
    resolve(null);
    await rejection;
    live.close();
    ydoc.destroy();
  }
});

for (const schedule of [
  "already-aborted",
  "empty",
  "resolve",
  "reject",
  "race",
  "abort-late-reject",
] as const) {
  await test(`owned preview ${schedule} settles and removes its abort listener without publishing a retired snapshot`, async () => {
    const ydoc = tiptapJsonToYDoc({
      type: "doc",
      content:
        schedule === "empty"
          ? [{ type: "paragraph", attrs: { id: "preview-empty" } }]
          : [{ type: "embed", attrs: { id: "preview-ref", entity: "document", ref: "target" } }],
    });
    const live = liveEditor(ydoc);
    const before = Y.encodeStateAsUpdate(ydoc);
    const { controller, listeners, counts } = previewAbortListenerWitness();
    const reason = new Error("preview scope retired");
    const serializeReached = new Error("normal producer reached actual DOM boundary");
    let serializations = 0;
    let resolverCalls = 0;
    Object.defineProperty(live.editor.view, "dom", {
      value: {
        ownerDocument: {
          createElement() {
            serializations++;
            throw serializeReached;
          },
        },
      },
    });
    let resolve: (value: EntitySnapshot | null) => void = () => {};
    let reject: (error: Error) => void = () => {};
    const hostPending = new Promise<EntitySnapshot | null>((done, fail) => {
      resolve = done;
      reject = fail;
    });
    if (schedule === "already-aborted") controller.abort(reason);
    const rendering = editorModePreview(
      live.editor,
      {
        entityResolver: () => {
          resolverCalls++;
          if (schedule === "race") controller.abort(reason);
          return hostPending;
        },
      },
      controller.signal,
    );
    const aborted =
      schedule === "already-aborted" || schedule === "race" || schedule === "abort-late-reject";
    const rejection = assert.rejects(
      rendering,
      (error: unknown) => error === (aborted ? reason : serializeReached),
    );
    try {
      if (schedule === "abort-late-reject") controller.abort(reason);
      if (schedule === "reject" || schedule === "abort-late-reject")
        reject(new Error("late host metadata failure"));
      else resolve({ label: "current target", icon: "" });
      await rejection;
      for (let index = 0; index < 5; index++) await Promise.resolve();
      assert.equal(listeners.size, 0);
      const subscriptions = schedule === "already-aborted" ? 0 : 1;
      assert.deepEqual(counts, { added: subscriptions, removed: subscriptions });
      assert.equal(serializations, aborted ? 0 : 1);
      assert.equal(resolverCalls, schedule === "already-aborted" || schedule === "empty" ? 0 : 1);
      assert.deepEqual(Y.encodeStateAsUpdate(ydoc), before);
      assert.equal(live.manager.undoStack.length, 0);
    } finally {
      resolve(null);
      await rejection;
      live.close();
      ydoc.destroy();
    }
  });
}

for (const reason of ["exact arbitrary reason", { source: "exact object reason" }]) {
  for (const schedule of [
    "entry",
    "late-resolve",
    "late-reject",
    "synchronous-resolver-abort",
    "settlement-race",
  ] as const) {
    await test(`actual preview ${typeof reason} reason identity and once-only cleanup during ${schedule}`, async () => {
      const doc = tiptapJsonToYDoc({
        type: "doc",
        content: [
          { type: "embed", attrs: { id: "exact-reason-ref", entity: "document", ref: "target" } },
        ],
      });
      const live = liveEditor(doc);
      const pm = live.editor.state.doc;
      const fragment = doc.getXmlFragment("prosemirror");
      const before = Y.encodeStateAsUpdate(doc);
      const undo = [...live.manager.undoStack];
      let updates = 0;
      doc.on("update", () => {
        updates++;
      });
      let serializations = 0;
      Object.defineProperty(live.editor.view, "dom", {
        value: {
          ownerDocument: {
            createElement() {
              serializations++;
              throw new Error("retired producer reached DOM");
            },
          },
        },
      });
      const { controller, listeners, counts } = previewAbortListenerWitness();
      let resolve: (value: EntitySnapshot | null) => void = () => {};
      let reject: (error: Error) => void = () => {};
      const shared = new Promise<EntitySnapshot | null>((done, fail) => {
        resolve = done;
        reject = fail;
      });
      if (schedule === "entry") controller.abort(reason);
      const pending = editorModePreview(
        live.editor,
        {
          entityResolver() {
            if (schedule === "synchronous-resolver-abort") controller.abort(reason);
            return shared;
          },
        },
        controller.signal,
      );
      let received = false;
      const rejected = pending.catch((error: unknown) => {
        assert.strictEqual(error, reason);
        received = true;
      });
      try {
        if (schedule === "settlement-race") resolve({ label: "completed host", icon: "" });
        if (schedule !== "entry" && schedule !== "synchronous-resolver-abort")
          controller.abort(reason);
        for (let index = 0; index < 5; index++) await Promise.resolve();
        assert.equal(received, true, "owned producer retires independently of shared IO");
        if (schedule === "late-reject") reject(new Error("late shared rejection"));
        else resolve({ label: "late host", icon: "" });
        await rejected;
        for (let index = 0; index < 5; index++) await Promise.resolve();
        const subscriptions = schedule === "entry" ? 0 : 1;
        assert.deepEqual(counts, { added: subscriptions, removed: subscriptions });
        assert.equal(listeners.size, 0);
        assert.equal(serializations, 0);
        assert.equal(updates, 0);
        assert.equal(live.editor.state.doc, pm);
        assert.equal(doc.getXmlFragment("prosemirror"), fragment);
        assert.deepEqual(Y.encodeStateAsUpdate(doc), before);
        assert.deepEqual(live.manager.undoStack, undo);
      } finally {
        resolve(null);
        await rejected;
        live.close();
        doc.destroy();
      }
    });
  }
}

await test("actual shell composition capture guards native NodeView targets, provisional 229 and retired owners without PM/Yjs writes", () => {
  const source = readFileSync(new URL("../src/vue/FvociEditor.vue", import.meta.url), "utf8");
  const script = source.split('<script setup lang="ts">')[1]?.split("</script>")[0];
  assert.ok(script);
  const parsed = ts.createSourceFile("FvociEditor.ts", script, ts.ScriptTarget.Latest, true);
  const names = new Set([
    "richComposing",
    "richCompositionTarget",
    "richCompositionStarted",
    "sourceBlocked",
    "retireRichComposition",
    "ownsRichInput",
    "onRichCompositionStart",
    "onRichCompositionEnd",
    "onRichKeyDown",
    "onRichKeyUp",
  ]);
  const statements = parsed.statements.filter((statement) => {
    if (ts.isFunctionDeclaration(statement)) return names.has(statement.name?.text ?? "");
    if (ts.isVariableStatement(statement))
      return statement.declarationList.declarations.some(
        (declaration) => ts.isIdentifier(declaration.name) && names.has(declaration.name.text),
      );
    return false;
  });
  assert.equal(statements.length, names.size);
  const scopeWatch = parsed.statements.find(
    (statement) =>
      ts.isExpressionStatement(statement) &&
      ts.isCallExpression(statement.expression) &&
      statement.expression.expression.getText(parsed) === "watch" &&
      statement.expression.arguments[0]?.getText(parsed).includes("props.modeScope"),
  );
  assert.ok(scopeWatch);
  const ydoc = tiptapJsonToYDoc(input);
  const live = liveEditor(ydoc);
  const before = Y.encodeStateAsUpdate(ydoc);
  class ControlledNode {
    isConnected = true;
  }
  const target = new ControlledNode();
  const other = new ControlledNode();
  const retired = new ControlledNode();
  retired.isConnected = false;
  const host = { contains: (node: ControlledNode) => node === target || node === other };
  const visible = Vue.ref(true);
  const stale = Vue.ref(false);
  const props = Vue.reactive({
    modeScope: 1,
    user: { id: "same-actor" },
    ydoc: Vue.markRaw(ydoc),
    provider: Vue.markRaw({}),
    editable: true,
  });
  const effects = Vue.effectScope();
  const controls = effects.run(
    () =>
      runInNewContext(
        ts.transpileModule(
          `(()=>{let previewAbort=null,scopeEpoch=0,modeLifetime=0;${statements.map((statement) => statement.getText(parsed)).join("\n")};${scopeWatch.getText(parsed)};return {onRichCompositionStart,onRichCompositionEnd,onRichKeyDown,onRichKeyUp,retireRichComposition,sourceBlocked,composing:()=>richComposing.value};})()`,
          { compilerOptions: { target: ts.ScriptTarget.ES2023, module: ts.ModuleKind.ESNext } },
        ).outputText,
        {
          ref: Vue.ref,
          watch: Vue.watch,
          props,
          mode: Vue.ref("rich"),
          sourceStale: stale,
          capture: Vue.shallowRef({}),
          richVisible: visible,
          editor: Vue.shallowRef(live.editor),
          host: Vue.shallowRef(host),
          sourceComposing: Vue.ref(false),
          Node: ControlledNode,
        },
      ) as unknown,
  ) as {
    onRichCompositionStart(event: unknown): void;
    onRichCompositionEnd(event: unknown): void;
    onRichKeyDown(event: unknown): void;
    onRichKeyUp(event: unknown): void;
    retireRichComposition(): void;
    sourceBlocked(): boolean;
    composing(): boolean;
  };
  const event = (node = target) => ({
    target: node,
    currentTarget: host,
    isComposing: false,
    keyCode: 0,
  });
  try {
    controls.onRichKeyDown({ ...event(), keyCode: 229 });
    assert.equal(controls.sourceBlocked(), true);
    controls.onRichKeyUp(event(other));
    assert.equal(controls.composing(), true);
    controls.onRichKeyUp(event());
    assert.equal(controls.sourceBlocked(), false);
    controls.onRichCompositionStart(event());
    // Native NodeView fields do not set PM view.composing, yet retain their
    // own composition through ordinary keyup and another field's late end.
    assert.equal(live.editor.view.composing, false);
    controls.onRichKeyUp(event());
    controls.onRichCompositionEnd(event(other));
    assert.equal(controls.sourceBlocked(), true);
    props.modeScope++;
    assert.equal(stale.value, true);
    assert.equal(controls.sourceBlocked(), true);
    assert.equal(controls.composing(), true);
    controls.onRichCompositionEnd(event());
    assert.equal(controls.sourceBlocked(), false);
    live.host.composing = true;
    assert.equal(controls.sourceBlocked(), true);
    live.host.composing = false;
    visible.value = false;
    controls.onRichCompositionStart(event());
    assert.equal(controls.sourceBlocked(), false);
    visible.value = true;
    controls.onRichCompositionStart(event(retired));
    assert.equal(controls.sourceBlocked(), false);
    controls.onRichCompositionStart(event());
    props.editable = false;
    assert.equal(controls.sourceBlocked(), false);
    props.editable = true;
    controls.onRichCompositionStart(event(other));
    controls.onRichCompositionEnd(event());
    assert.equal(controls.sourceBlocked(), true);
    controls.onRichCompositionEnd(event(other));
    assert.equal(controls.sourceBlocked(), false);
    controls.onRichCompositionStart(event());
    controls.retireRichComposition();
    controls.onRichCompositionEnd(event());
    assert.equal(controls.sourceBlocked(), false);
    assert.deepEqual(Y.encodeStateAsUpdate(ydoc), before);
    assert.equal(live.manager.undoStack.length, 0);
  } finally {
    effects.stop();
    live.close();
    ydoc.destroy();
  }
});

await test("actual mode entry refreshes a clean generated projection after rich edits but preserves an older private draft without writes", async () => {
  const text = readFileSync(new URL("../src/vue/FvociEditor.vue", import.meta.url), "utf8");
  const script = text.split('<script setup lang="ts">')[1]?.split("</script>")[0];
  assert.ok(script);
  const parsed = ts.createSourceFile("FvociEditor.ts", script, ts.ScriptTarget.Latest, true);
  const statements = parsed.statements.filter(
    (statement) =>
      ts.isFunctionDeclaration(statement) &&
      ["changeMode", "refreshSource"].includes(statement.name?.text ?? ""),
  );
  assert.equal(statements.length, 2);
  const ydoc = tiptapJsonToYDoc(input);
  const live = liveEditor(ydoc);
  const source = new SourceModeSession(
    ydoc,
    () => 1,
    () => true,
  );
  const mode = Vue.ref("rich");
  const capture = Vue.shallowRef(source.capture(live.editor.state.doc));
  const field = { value: capture.value.source, focus() {} };
  const dirty = Vue.ref(false);
  const stale = Vue.ref(false);
  const controls = runInNewContext(
    ts.transpileModule(
      `(()=>{let previewAbort=null,modeLifetime=0,bookmark=null,storedMarks=null,restoreEditorFocus=false;${statements.map((statement) => statement.getText(parsed)).join("\n")};return {changeMode};})()`,
      { compilerOptions: { target: ts.ScriptTarget.ES2023, module: ts.ModuleKind.ESNext } },
    ).outputText,
    {
      editor: Vue.shallowRef(live.editor),
      sourceSession: source,
      sourceBlocked: () => false,
      mode,
      richVisible: Vue.computed(() => mode.value === "rich" || mode.value === "block"),
      capture,
      sourceField: Vue.shallowRef(field),
      draftDirty: dirty,
      sourceStale: stale,
      proposal: Vue.shallowRef(null),
      modeError: Vue.ref(null),
      emit() {},
      nextTick: Vue.nextTick,
    },
  ) as { changeMode(next: string): Promise<void> };
  try {
    live.editor.view.dispatch(live.editor.state.tr.insertText(" rich", 1));
    const afterRich = Y.encodeStateAsUpdate(ydoc);
    await controls.changeMode("markdown");
    assert.equal(field.value, " rich한글 연구\n\n자료");
    assert.equal(source.isCurrent(capture.value), true);
    assert.deepEqual(Y.encodeStateAsUpdate(ydoc), afterRich);
    mode.value = "rich";
    const peerDoc = new Y.Doc({ gc: false });
    Y.applyUpdate(peerDoc, Y.encodeStateAsUpdate(ydoc));
    const peer = liveEditor(peerDoc);
    const receive = (update: Uint8Array) => {
      Y.applyUpdate(ydoc, update);
    };
    peerDoc.on("update", receive);
    try {
      peer.editor.view.dispatch(peer.editor.state.tr.insertText(" peer", 1));
      const afterPeer = Y.encodeStateAsUpdate(ydoc);
      await controls.changeMode("markdown");
      assert.equal(field.value, " peer rich한글 연구\n\n자료");
      assert.equal(source.isCurrent(capture.value), true);
      assert.deepEqual(Y.encodeStateAsUpdate(ydoc), afterPeer);
    } finally {
      peerDoc.off("update", receive);
      peer.close();
      peerDoc.destroy();
    }
    mode.value = "rich";
    const privateCapture = capture.value;
    field.value = "직접 작성한 비공개 초안 🧑‍💻";
    dirty.value = true;
    live.editor.view.dispatch(live.editor.state.tr.insertText(" newer", 1));
    const afterNewer = Y.encodeStateAsUpdate(ydoc);
    stale.value = true;
    await controls.changeMode("markdown");
    assert.equal(field.value, "직접 작성한 비공개 초안 🧑‍💻");
    assert.equal(capture.value, privateCapture);
    assert.equal(source.isCurrent(capture.value), false);
    assert.equal(stale.value, true);
    assert.deepEqual(Y.encodeStateAsUpdate(ydoc), afterNewer);
  } finally {
    source.destroy();
    live.close();
    ydoc.destroy();
  }
});

await test("installed SDK XML serialization does not normalize raw future node identity, and parser emoji semantics have independent mutant controls", () => {
  const doc = new Y.Doc({ gc: false });
  try {
    const future = new Y.XmlElement("futureNode");
    future.setAttribute("id", "future-preserved");
    const text = new Y.XmlText();
    text.insert(0, "한글 🧑‍💻");
    future.insert(0, [text]);
    doc.getXmlFragment("prosemirror").insert(0, [future]);
    assert.equal(future.nodeName, "futureNode");
    assert.equal(
      doc.getXmlFragment("prosemirror").toJSON(),
      '<futurenode id="future-preserved">한글 🧑‍💻</futurenode>',
    );
    const expected = {
      type: "doc",
      content: [
        {
          type: "futureNode",
          attrs: { id: "future-preserved" },
          content: [{ type: "text", text: "한글 🧑‍💻" }],
        },
      ],
    };
    assert.deepEqual(yDocToTiptapJson(doc), expected);
    for (const field of ["type", "id"]) {
      const mutant = structuredClone(expected);
      const node = mutant.content[0];
      assert.ok(node);
      if (field === "type") node.type = "futurenode";
      else node.attrs.id = "different-ref";
      assert.throws(() => {
        assert.deepEqual(mutant, expected);
      });
    }
    assert.deepEqual(mdToTiptapJson("가장 최신 수정 🧑‍💻"), {
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text: "가장 최신 수정 🧑‍💻" }] }],
    });
    const semantic = [
      { type: "text", text: "가장 최신 수정 " },
      { type: "emoji", attrs: { name: "technologist" } },
    ];
    const liveDoc = tiptapJsonToYDoc({
      type: "doc",
      content: [
        { type: "paragraph", attrs: { id: "oracle-p" }, content: [{ type: "text", text: "원본" }] },
      ],
    });
    const live = liveEditor(liveDoc);
    const source = new SourceModeSession(
      liveDoc,
      () => 1,
      () => true,
    );
    try {
      const capture = source.capture(live.editor.state.doc);
      assert.equal(
        source.apply(source.prepare(capture, "가장 최신 수정 🧑‍💻", live.editor.state), live.editor),
        true,
      );
      // The parser preserves Unicode text; the installed ordinary Emoji
      // appendTransaction represents unmarked Unicode as its existing atom.
      assert.deepEqual(yDocToTiptapJson(liveDoc), {
        type: "doc",
        content: [{ type: "paragraph", attrs: { id: "oracle-p" }, content: semantic }],
      });
    } finally {
      source.destroy();
      live.close();
      liveDoc.destroy();
    }
    assert.throws(() => {
      assert.deepEqual([{ type: "text", text: "가장 최신 수정 " }], semantic);
    });
    assert.throws(() => {
      assert.deepEqual(
        [
          { type: "text", text: "가장 최신 수정 " },
          { type: "emoji", attrs: { name: "different-emoji" } },
        ],
        semantic,
      );
    });
    assert.throws(() => {
      assert.deepEqual(
        [
          { type: "text", text: "different text " },
          { type: "emoji", attrs: { name: "technologist" } },
        ],
        semantic,
      );
    });
  } finally {
    doc.destroy();
  }
});

function liveEditor(ydoc: Y.Doc) {
  const editor = new Editor({
    element: null,
    extensions: createFvociExtensions(),
    content: { type: "doc", content: [{ type: "paragraph" }] },
  });
  const fragment = ydoc.getXmlFragment("prosemirror");
  const initial = initProseMirrorDoc(fragment, editor.schema);
  const sync = ySyncPlugin(fragment, { mapping: initial.mapping }) as Plugin;
  const manager = new Y.UndoManager(fragment, { trackedOrigins: new Set([ySyncPluginKey]) });
  let state = EditorState.create({
    schema: editor.schema,
    doc: initial.doc,
    plugins: [
      sync,
      yUndoPlugin({ undoManager: manager }) as Plugin,
      ...editor.extensionManager.plugins,
    ],
  });
  const lifecycle: { binding?: ReturnType<NonNullable<Plugin["spec"]["view"]>> } = {};
  const host = {
    state,
    composing: false,
    get editable() {
      return editor.options.editable;
    },
    hasFocus: () => false,
    dispatch(tr: Transaction) {
      const before = state;
      state = state.applyTransaction(tr).state;
      host.state = state;
      lifecycle.binding?.update?.(host as EditorView, before);
    },
  };
  Object.defineProperty(editor, "state", { configurable: true, get: () => state });
  Object.defineProperty(editor, "view", { configurable: true, value: host });
  // The SDK's headless Editor has no editorView and reports isDestroyed=true.
  // This test controls only the host flags; sync/transactions/CRDT are real.
  Object.defineProperty(editor, "isDestroyed", { configurable: true, value: false });
  assert.ok(sync.spec.view);
  lifecycle.binding = sync.spec.view(host as EditorView);
  return {
    editor,
    manager,
    host,
    close() {
      lifecycle.binding?.destroy?.();
      manager.destroy();
      Reflect.deleteProperty(editor, "view");
      Reflect.deleteProperty(editor, "state");
      Reflect.deleteProperty(editor, "isDestroyed");
      editor.destroy();
    },
  };
}

await test("actual block-math watcher and commands make zero readonly or retired writes, retain a readable draft and refuse to overwrite a peer's newer latex", async () => {
  const doc = tiptapJsonToYDoc({
    type: "doc",
    content: [
      { type: "math", attrs: { id: "math-owner", latex: "x + y" } },
      { type: "paragraph", attrs: { id: "math-tail" } },
    ],
  });
  const peerDoc = new Y.Doc({ gc: false });
  Y.applyUpdate(peerDoc, Y.encodeStateAsUpdate(doc));
  const local = liveEditor(doc);
  const peer = liveEditor(peerDoc);
  const wire = Symbol("wire");
  doc.on("update", (value, origin) => {
    if (origin !== wire) Y.applyUpdate(peerDoc, value, wire);
  });
  peerDoc.on("update", (value, origin) => {
    if (origin !== wire) Y.applyUpdate(doc, value, wire);
  });
  local.host.dispatch(
    local.editor.state.tr.setSelection(NodeSelection.create(local.editor.state.doc, 0)),
  );
  const props: {
    editor: Editor;
    node: PmNode;
    getPos: () => number | undefined;
    updateAttributes: (attrs: Record<string, unknown>) => void;
  } = Vue.reactive({
    // Installed VueRenderer deep-reactivates NodeView props; its Vue Editor
    // is markRaw. Exercise that actual proxy boundary, not shallow props.
    editor: Vue.markRaw(local.editor),
    node: local.editor.state.doc.child(0),
    getPos: () => 0,
    updateAttributes: (attrs: Record<string, unknown>) => {
      assert.equal(local.editor.commands.updateAttributes("math", attrs), true);
      props.node = local.editor.state.doc.child(0);
    },
  });
  assert.equal(Vue.isProxy(props.node), true);
  assert.notEqual(props.node.type, local.editor.state.doc.child(0).type);
  assert.equal(Vue.toRaw(props.node.type), local.editor.state.doc.child(0).type);
  const unmount: (() => void)[] = [];
  const field = { value: "", focus() {} };
  const input = Vue.shallowRef(field);
  const editableSource = readFileSync(
    new URL("../src/vue/use-editable.ts", import.meta.url),
    "utf8",
  );
  const mathSource = readFileSync(new URL("../src/vue/MathNodeView.vue", import.meta.url), "utf8");
  const mathScript = mathSource.split('<script setup lang="ts">')[1]?.split("</script>")[0];
  assert.ok(mathScript);
  // Execute the actual consumer and editable owner, without a copied watcher.
  // Only DOM refs/render/lifecycle are controlled; PM commands and Yjs are real.
  const code =
    editableSource
      .slice(editableSource.indexOf("export function useEditable"))
      .replace("export function", "function") +
    "\n" +
    mathScript.slice(mathScript.indexOf("const editable ="));
  type Controls = {
    open(): Promise<void>;
    onInput(event: Event): void;
    onBlur(event: FocusEvent): void;
    cancel(): void;
    privateDraft(): string | null;
  };
  const effects = Vue.effectScope();
  const controls = effects.run(
    () =>
      runInNewContext(
        ts.transpileModule(
          `(()=>{${code};return {open,onInput,onBlur,cancel,privateDraft:()=>draft.value};})()`,
          {
            compilerOptions: { target: ts.ScriptTarget.ES2023, module: ts.ModuleKind.ESNext },
          },
        ).outputText,
        {
          ...Vue,
          props,
          t: (key: string) => key,
          useTemplateRef: () => input,
          useMathMl: () => ({ html: null, failed: false }),
          onBeforeUnmount: (callback: () => void) => unmount.push(callback),
        },
      ) as unknown as Controls,
  );
  assert.ok(controls);
  const event = (type: string, target = field) => {
    const value = new Event(type);
    Object.defineProperty(value, "target", { value: target });
    return value;
  };
  let updates = 0;
  doc.on("update", () => {
    updates++;
  });
  try {
    await controls.open();
    assert.equal(field.value, "x + y");
    field.value = "private pending";
    controls.onInput(event("input"));
    const before = Y.encodeStateAsUpdate(doc);
    local.editor.setEditable(false);
    await Vue.nextTick();
    assert.equal(local.editor.isEditable, false);
    assert.equal(local.editor.state.doc.child(0).attrs.latex, "x + y");
    assert.equal(updates, 0);
    controls.onBlur(event("blur") as FocusEvent);
    assert.equal(updates, 0);
    assert.deepEqual(Y.encodeStateAsUpdate(doc), before);
    local.editor.setEditable(true);
    await Vue.nextTick();
    // Reauthorization alone cannot restore the old removed field's authority.
    controls.onBlur(event("blur") as FocusEvent);
    assert.equal(updates, 0);
    assert.deepEqual(Y.encodeStateAsUpdate(doc), before);
    assert.equal(controls.privateDraft(), "private pending");
    controls.cancel();
    const newField = { value: "", focus() {} };
    input.value = newField;
    await controls.open();
    newField.value = "current private";
    controls.onInput(event("input", newField));
    field.value = "cancelled old field value";
    controls.onInput(event("input"));
    controls.onBlur(event("blur") as FocusEvent);
    assert.equal(controls.privateDraft(), "current private");
    assert.equal(updates, 0);
    assert.deepEqual(Y.encodeStateAsUpdate(doc), before);
    controls.cancel();
    let focused = 0;
    const raceField = {
      value: "",
      focus() {
        focused++;
      },
    };
    input.value = raceField;
    const retiredOpen = controls.open();
    controls.cancel();
    const currentOpen = controls.open();
    await Promise.all([retiredOpen, currentOpen]);
    assert.equal(focused, 1);
    assert.equal(raceField.value, "x + y");
    assert.equal(updates, 0);
    controls.cancel();
    input.value = field;
    await controls.open();
    field.value = "private pending";
    controls.onInput(event("input"));
    local.editor.setEditable(false);
    await Vue.nextTick();
    local.editor.setEditable(true);
    await Vue.nextTick();
    await controls.open();
    assert.equal(field.value, "private pending");
    field.value = "authorized z";
    controls.onInput(event("input"));
    controls.onBlur(event("blur") as FocusEvent);
    assert.equal(local.editor.state.doc.child(0).attrs.latex, "authorized z");
    assert.equal(local.editor.state.doc.child(0).attrs.id, "math-owner");
    assert.equal(peer.editor.state.doc.child(0).attrs.latex, "authorized z");
    assert.equal(updates, 1);
    await controls.open();
    field.value = "stale private";
    controls.onInput(event("input"));
    peer.host.dispatch(
      peer.editor.state.tr.setNodeMarkup(0, undefined, {
        ...peer.editor.state.doc.child(0).attrs,
        latex: "peer newest",
      }),
    );
    props.node = local.editor.state.doc.child(0);
    const afterPeer = Y.encodeStateAsUpdate(doc);
    const count = updates;
    controls.onBlur(event("blur") as FocusEvent);
    assert.equal(local.editor.state.doc.child(0).attrs.latex, "peer newest");
    assert.equal(peer.editor.state.doc.child(0).attrs.latex, "peer newest");
    assert.equal(updates, count);
    assert.deepEqual(Y.encodeStateAsUpdate(doc), afterPeer);
    for (const callback of unmount) callback();
    field.value = "late retired callback";
    controls.onBlur(event("blur") as FocusEvent);
    assert.equal(updates, count);
    assert.deepEqual(Y.encodeStateAsUpdate(doc), afterPeer);
  } finally {
    effects.stop();
    local.close();
    peer.close();
    doc.destroy();
    peerDoc.destroy();
  }
});

const input = {
  type: "doc" as const,
  content: [
    { type: "paragraph", attrs: { id: "p" }, content: [{ type: "text", text: "한글 연구" }] },
    { type: "paragraph", attrs: { id: "other" }, content: [{ type: "text", text: "자료" }] },
  ],
};

await test("actual ySync localized source edit is one undo item and later peer text in the same paragraph survives undo/redo", () => {
  const localDoc = tiptapJsonToYDoc(input);
  const peerDoc = new Y.Doc({ gc: false });
  Y.applyUpdate(peerDoc, Y.encodeStateAsUpdate(localDoc));
  const local = liveEditor(localDoc);
  const peer = liveEditor(peerDoc);
  const remoteOrigin = Symbol("wire");
  localDoc.on("update", (update, origin) => {
    if (origin !== remoteOrigin) Y.applyUpdate(peerDoc, update, remoteOrigin);
  });
  peerDoc.on("update", (update, origin) => {
    if (origin !== remoteOrigin) Y.applyUpdate(localDoc, update, remoteOrigin);
  });
  const source = new SourceModeSession(
    localDoc,
    () => 1,
    () => true,
  );
  try {
    const capture = source.capture(local.editor.state.doc);
    const proposal = source.prepare(
      capture,
      capture.source.replace("연구", "조사"),
      local.editor.state,
    );
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.equal(source.apply(proposal, local.editor), true);
    assert.equal(local.manager.undoStack.length, 1);
    assert.equal(peer.editor.state.doc.child(0).textContent, "한글 조사");
    peer.host.dispatch(peer.editor.state.tr.insertText(" 동료", 6));
    assert.equal(local.editor.state.doc.child(0).textContent, "한글 조사 동료");
    local.manager.undo();
    assert.equal(local.editor.state.doc.child(0).textContent, "한글 연구 동료");
    assert.equal(peer.editor.state.doc.child(0).textContent, "한글 연구 동료");
    assert.equal(local.editor.state.doc.child(0).attrs.id, "p");
    local.manager.redo();
    assert.equal(local.editor.state.doc.child(0).textContent, "한글 조사 동료");
    assert.deepEqual(yDocToTiptapJson(localDoc), yDocToTiptapJson(peerDoc));
  } finally {
    source.destroy();
    local.close();
    peer.close();
    localDoc.destroy();
    peerDoc.destroy();
  }
});

await test("permission revoke, composition and retired source apply cannot publish a Yjs update", () => {
  const ydoc = tiptapJsonToYDoc(input);
  const local = liveEditor(ydoc);
  let authorized = true;
  const source = new SourceModeSession(
    ydoc,
    () => 1,
    () => authorized,
  );
  try {
    const capture = source.capture(local.editor.state.doc);
    const proposal = source.prepare(
      capture,
      capture.source.replace("연구", "조사"),
      local.editor.state,
    );
    const before = Y.encodeStateAsUpdate(ydoc);
    local.host.composing = true;
    assert.equal(source.apply(proposal, local.editor), false);
    local.host.composing = false;
    authorized = false;
    assert.equal(source.apply(proposal, local.editor), false);
    authorized = true;
    source.destroy();
    assert.equal(source.apply(proposal, local.editor), false);
    assert.deepEqual(Y.encodeStateAsUpdate(ydoc), before);
    assert.equal(local.manager.undoStack.length, 0);
  } finally {
    source.destroy();
    local.close();
    ydoc.destroy();
  }
});

await test("backward and node bookmarks map through localized edits without changing content or forcing end selection", () => {
  const ydoc = tiptapJsonToYDoc(input);
  const local = liveEditor(ydoc);
  const source = new SourceModeSession(
    ydoc,
    () => 1,
    () => true,
  );
  try {
    const bookmark = TextSelection.create(local.editor.state.doc, 5, 2).getBookmark();
    local.host.dispatch(
      local.editor.state.tr
        .setSelection(TextSelection.create(local.editor.state.doc, 2))
        .setStoredMarks([local.editor.schema.mark("bold")]),
    );
    const capture = source.capture(local.editor.state.doc);
    const proposal = source.prepare(
      capture,
      capture.source.replace("연구", "긴 조사"),
      local.editor.state,
    );
    assert.ok(proposal.transaction);
    const mapped = bookmark.map(proposal.transaction.mapping).resolve(proposal.transaction.doc);
    assert.ok(mapped.anchor > mapped.head);
    assert.equal(mapped.head, 2);
    assert.equal(proposal.transaction.storedMarks?.[0]?.type.name, "bold");
    const node = NodeSelection.create(
      local.editor.state.doc,
      local.editor.state.doc.child(0).nodeSize,
    );
    const nodeMapped = node
      .getBookmark()
      .map(proposal.transaction.mapping)
      .resolve(proposal.transaction.doc);
    assert.equal(nodeMapped instanceof NodeSelection, true);
    assert.equal(nodeMapped.$from.nodeAfter?.attrs.id, "other");
  } finally {
    source.destroy();
    local.close();
    ydoc.destroy();
  }
});

await test("preview SafeHtml producer retains supported presentation while removing active hostile HTML", () => {
  const preview = sanitizeEditorModePreview(
    '<p><u>&lt;script&gt;evil()&lt;/script&gt; 한글</u><a href="javascript:evil()" onclick="evil()">링크</a></p><details><summary>요약</summary><p>내용</p></details><script>active()</script>',
  ).html;
  assert.ok(preview.includes("&lt;script&gt;evil()&lt;/script&gt; 한글"));
  assert.ok(preview.includes("<u>"));
  assert.ok(preview.includes("<details"));
  assert.ok(preview.includes("요약") && preview.includes("내용"));
  assert.equal(preview.includes("<script>"), false);
  assert.equal(preview.includes("javascript:"), false);
  assert.equal(preview.includes("onclick"), false);
  assert.equal(preview.includes("active()"), false);
});

await test("untouched raw extension attributes survive a real localized ySync source edit of another block", () => {
  const ydoc = tiptapJsonToYDoc(input);
  const local = liveEditor(ydoc);
  const source = new SourceModeSession(
    ydoc,
    () => 1,
    () => true,
  );
  try {
    const fragment = ydoc.getXmlFragment("prosemirror");
    const opaque = fragment.get(1);
    assert.ok(opaque instanceof Y.XmlElement);
    opaque.setAttribute("futureFlag", "keep-me");
    const capture = source.capture(local.editor.state.doc);
    const proposal = source.prepare(
      capture,
      capture.source.replace("연구", "조사"),
      local.editor.state,
    );
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.equal(source.apply(proposal, local.editor), true);
    assert.equal(opaque.getAttribute("futureFlag"), "keep-me");
    assert.equal(yDocToTiptapJson(ydoc).content?.length, 2);
    const changedCapture = source.capture(local.editor.state.doc);
    const refused = source.prepare(
      changedCapture,
      changedCapture.source.replace("자료", "노트"),
      local.editor.state,
    );
    assert.equal(refused.status, "loss");
    assert.equal(refused.diagnostics[0]?.id, "other");
    assert.equal(refused.diagnostics[0].field, "attrs.futureFlag");
  } finally {
    source.destroy();
    local.close();
    ydoc.destroy();
  }
});

function addFuture(peer: Y.Doc, kind: "node" | "mark"): void {
  if (kind === "node") {
    const node = new Y.XmlElement("futureNode");
    node.setAttribute("id", "future-id");
    const text = new Y.XmlText();
    text.insert(0, "미래 원본 🧑‍💻");
    node.insert(0, [text]);
    peer.getXmlFragment("prosemirror").insert(1, [node]);
  } else {
    const paragraph = peer.getXmlFragment("prosemirror").get(0);
    assert.ok(paragraph instanceof Y.XmlElement);
    const text = paragraph.get(0);
    assert.ok(text instanceof Y.XmlText);
    text.format(0, 2, { futureMark: { ref: "future-ref" } });
  }
}

await test("negative control: observer-only 096 foundation binding deletes a late schema-unknown node", () => {
  const ydoc = tiptapJsonToYDoc(input);
  const peer = new Y.Doc({ gc: false });
  Y.applyUpdate(peer, Y.encodeStateAsUpdate(ydoc));
  const source = new SourceModeSession(
    ydoc,
    () => 1,
    () => true,
  );
  const local = liveEditor(ydoc);
  try {
    const capture = source.capture(local.editor.state.doc);
    addFuture(peer, "node");
    const expected = yDocToTiptapJson(peer);
    Y.applyUpdate(ydoc, Y.encodeStateAsUpdate(peer), "remote");
    assert.notDeepEqual(yDocToTiptapJson(ydoc), expected);
    assert.equal(yDocToTiptapJson(ydoc).content?.length, 2);
    assert.equal(source.isCurrent(capture), false);
  } finally {
    source.destroy();
    local.close();
    ydoc.destroy();
    peer.destroy();
  }
});

for (const kind of ["node", "mark"] as const) {
  await test(`earlier raw observer retires binding before late unsupported ${kind} can cause an SDK repair write`, () => {
    const ydoc = tiptapJsonToYDoc(input);
    const peer = new Y.Doc({ gc: false });
    Y.applyUpdate(peer, Y.encodeStateAsUpdate(ydoc));
    const holder: { local: ReturnType<typeof liveEditor> | null } = { local: null };
    let retired = 0;
    let repairs = 0;
    const source = new SourceModeSession(
      ydoc,
      () => 1,
      () => true,
      () => {
        if (!holder.local) return;
        const diagnostics = rawEditorPreflight(ydoc, holder.local.editor.schema);
        if (!diagnostics.length) return;
        retired++;
        // Real SDK view.destroy and owned manager lifecycle, corresponding to
        // the mounted editor.destroy path. Physical DOM teardown is E2E scope.
        holder.local.close();
        holder.local = null;
      },
    );
    holder.local = liveEditor(ydoc);
    ydoc.on("update", (_update, origin) => {
      if (origin === ySyncPluginKey) repairs++;
    });
    try {
      const capture = source.capture(holder.local.editor.state.doc);
      const schema = holder.local.editor.schema;
      peer.transact(() => {
        addFuture(peer, kind);
        const fragment = peer.getXmlFragment("prosemirror");
        const supported = fragment.get(fragment.length - 1);
        assert.ok(supported instanceof Y.XmlElement);
        const text = supported.get(0);
        assert.ok(text instanceof Y.XmlText);
        text.insert(text.length, " 동시 편집");
      }, "older-schema-peer");
      const expected = yDocToTiptapJson(peer);
      const peerSnapshot = Y.snapshot(peer);
      Y.applyUpdate(ydoc, Y.encodeStateAsUpdate(peer), "remote");
      assert.equal(retired, 1);
      assert.equal(repairs, 0);
      assert.deepEqual(yDocToTiptapJson(ydoc), expected);
      assert.ok(Y.equalSnapshots(Y.snapshot(ydoc), peerSnapshot));
      assert.equal(source.isCurrent(capture), false);
      const supported = peer
        .getXmlFragment("prosemirror")
        .get(peer.getXmlFragment("prosemirror").length - 1);
      assert.ok(supported instanceof Y.XmlElement);
      const text = supported.get(0);
      assert.ok(text instanceof Y.XmlText);
      text.insert(text.length, " 이후 편집");
      Y.applyUpdate(ydoc, Y.encodeStateAsUpdate(peer), "remote");
      assert.equal(retired, 1);
      assert.equal(repairs, 0);
      assert.deepEqual(yDocToTiptapJson(ydoc), yDocToTiptapJson(peer));
      assert.equal(
        source.prepare(
          capture,
          "stale overwrite",
          EditorState.create({ schema, doc: schema.nodeFromJSON(input) }),
        ).status,
        "stale",
      );
    } finally {
      source.destroy();
      const close = (local: ReturnType<typeof liveEditor> | null) => local?.close();
      close(holder.local);
      ydoc.destroy();
      peer.destroy();
    }
  });
}

await test("actual ySync paragraph split retains hidden presentation and undo keeps a later peer edit of the surviving first block", () => {
  const fixture = {
    type: "doc" as const,
    content: [
      {
        type: "paragraph",
        attrs: { id: "split", textAlign: "right" },
        content: [
          {
            type: "text",
            text: "alpha beta",
            marks: [{ type: "underline" }, { type: "textStyle", attrs: { color: "#112233" } }],
          },
        ],
      },
      {
        type: "paragraph",
        attrs: { id: "link-neighbor", textAlign: "center" },
        content: [
          {
            type: "text",
            text: "원본 블록 참조",
            marks: [{ type: "link", attrs: { href: "#split", title: "대상", target: "_self" } }],
          },
        ],
      },
      {
        type: "attachment",
        attrs: {
          id: "10000000-0000-4000-8000-000000000009",
          name: "자료.pdf",
          caption: "설명",
          image: false,
          width: 70,
          align: "left",
          previewWidth: 640,
          previewHeight: 480,
        },
      },
      {
        type: "embed",
        attrs: { id: "embed-neighbor", entity: "document", ref: "document-target" },
      },
      { type: "paragraph", attrs: { id: "closing" }, content: [{ type: "text", text: "끝" }] },
    ],
  };
  const localDoc = tiptapJsonToYDoc(fixture);
  const peerDoc = new Y.Doc({ gc: false });
  Y.applyUpdate(peerDoc, Y.encodeStateAsUpdate(localDoc));
  const local = liveEditor(localDoc);
  const peer = liveEditor(peerDoc);
  const wire = Symbol("wire");
  localDoc.on("update", (update, origin) => {
    if (origin !== wire) Y.applyUpdate(peerDoc, update, wire);
  });
  peerDoc.on("update", (update, origin) => {
    if (origin !== wire) Y.applyUpdate(localDoc, update, wire);
  });
  const source = new SourceModeSession(
    localDoc,
    () => 1,
    () => true,
  );
  const tails = [local, peer].map((client) =>
    client.editor.state.doc.content.cut(client.editor.state.doc.child(0).nodeSize),
  );
  let splitId: unknown;
  const stage = (texts: string[]) => {
    for (const [index, client] of [local, peer].entries()) {
      const doc = client.editor.state.doc;
      assert.equal(doc.childCount, texts.length + 4);
      assert.equal(doc.child(0).attrs.id, "split");
      if (texts.length === 2) assert.equal(doc.child(1).attrs.id, splitId);
      let tailStart = 0;
      for (const [i, text] of texts.entries()) {
        const block = doc.child(i);
        assert.equal(block.textContent, text);
        assert.equal(block.attrs.textAlign, "right");
        block.descendants((node) => {
          if (!node.isText) return;
          assert.equal(node.marks.length, 2);
          assert.ok(node.marks.some((mark) => mark.type.name === "underline"));
          assert.equal(
            node.marks.find((mark) => mark.type.name === "textStyle")?.attrs.color,
            "#112233",
          );
        });
        tailStart += block.nodeSize;
      }
      const tail = tails[index];
      assert.ok(tail);
      assert.ok(doc.content.cut(tailStart).eq(tail));
      assert.equal(doc.child(texts.length).child(0).marks[0]?.attrs.href, "#split");
      assert.equal(doc.child(texts.length + 1).attrs.id, "10000000-0000-4000-8000-000000000009");
      assert.equal(doc.child(texts.length + 2).attrs.ref, "document-target");
    }
    assert.ok(Y.equalSnapshots(Y.snapshot(localDoc), Y.snapshot(peerDoc)));
    assert.deepEqual(Y.encodeStateAsUpdate(localDoc), Y.encodeStateAsUpdate(peerDoc));
  };
  try {
    const capture = source.capture(local.editor.state.doc);
    const proposal = source.prepare(
      capture,
      capture.source.replace("alpha beta", "alpha\n\nbeta"),
      local.editor.state,
    );
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.equal(source.apply(proposal, local.editor), true);
    splitId = local.editor.state.doc.child(1).attrs.id;
    assert.equal(typeof splitId, "string");
    assert.ok(splitId && splitId !== "split");
    assert.equal(local.manager.undoStack.length, 1);
    assert.equal(peer.manager.undoStack.length, 0);
    stage(["alpha", "beta"]);
    peer.host.dispatch(peer.editor.state.tr.insertText(" peer", 6));
    stage(["alpha peer", "beta"]);
    const paragraph = peerDoc.getXmlFragment("prosemirror").get(0);
    assert.ok(paragraph instanceof Y.XmlElement);
    const text = paragraph.get(0);
    assert.ok(text instanceof Y.XmlText);
    const start = Y.createRelativePositionFromTypeIndex(text, 5, 0);
    const end = Y.createRelativePositionFromTypeIndex(text, 10, -1);
    assert.ok(start.item);
    assert.equal(start.item.client, peerDoc.clientID);
    const anchored = (from: number, to: number) => {
      for (const doc of [localDoc, peerDoc]) {
        const left = Y.createAbsolutePositionFromRelativePosition(start, doc);
        const right = Y.createAbsolutePositionFromRelativePosition(end, doc);
        assert.ok(left && right);
        assert.equal(left.index, from);
        assert.equal(right.index, to);
        assert.equal(left.type, right.type);
        assert.ok(left.type instanceof Y.XmlText);
        assert.ok(left.type.parent instanceof Y.XmlElement);
        assert.equal(left.type.parent.getAttribute("id"), "split");
        const delta: unknown = left.type.toDelta();
        assert.ok(Array.isArray(delta));
        const plain = delta
          .map((part: unknown) => {
            assert.ok(typeof part === "object" && part !== null && "insert" in part);
            assert.ok(typeof part.insert === "string");
            return part.insert;
          })
          .join("");
        const value = plain.slice(left.index, right.index);
        assert.equal(value, " peer");
        assert.equal(Buffer.from(value).toString("hex"), "2070656572");
        assert.ok(start.item);
        const item = Y.getItem(doc.store, start.item);
        assert.ok(item instanceof Y.Item);
        assert.equal(item.deleted, false);
        assert.equal(item.id.client, peerDoc.clientID);
      }
    };
    anchored(5, 10);
    assert.equal(peer.manager.undoStack.length, 1);
    local.manager.undo();
    // Independently assessed selective-history order: the old local suffix
    // resurrects before the same live peer item, whose relative identity stays.
    stage(["alpha beta peer"]);
    anchored(10, 15);
    assert.equal(local.manager.undoStack.length, 0);
    assert.equal(local.manager.redoStack.length, 1);
    assert.equal(peer.manager.undoStack.length, 1);
    local.manager.redo();
    stage(["alpha peer", "beta"]);
    anchored(5, 10);
    assert.equal(local.manager.undoStack.length, 1);
    assert.equal(local.manager.redoStack.length, 0);
    assert.equal(peer.manager.undoStack.length, 1);
    local.manager.undo();
    stage(["alpha beta peer"]);
    anchored(10, 15);
    assert.equal(peer.manager.undoStack.length, 1);
  } finally {
    source.destroy();
    local.close();
    peer.close();
    localDoc.destroy();
    peerDoc.destroy();
  }
});

await test("preview atom producer preserves actual file/name/caption and resolved entity/ref semantics instead of empty schema elements", () => {
  const ydoc = tiptapJsonToYDoc(input);
  const local = liveEditor(ydoc);
  try {
    const file = local.editor.schema.nodeFromJSON({
      type: "attachment",
      attrs: {
        id: "10000000-0000-4000-8000-000000000009",
        name: "%EC%9E%90%EB%A3%8C.pdf",
        caption: "<script>literal caption</script>",
        align: "right",
      },
    });
    const card = JSON.stringify(
      attachmentPreviewSpec(file, {
        upload: () => Promise.reject(new Error("Preview must not upload")),
        downloadUrl: (id) => `/api/v1/workspaces/ws/attachments/${id}/download`,
      }),
    );
    assert.ok(card.includes("자료.pdf"));
    assert.ok(card.includes("10000000-0000-4000-8000-000000000009/download"));
    assert.ok(card.includes("<script>literal caption</script>"));
    assert.ok(card.includes("right"));
    const withoutBridge = JSON.stringify(attachmentPreviewSpec(file));
    assert.equal(withoutBridge.includes('"href"'), false);
    assert.ok(withoutBridge.includes("자료.pdf"));
    const embedded = local.editor.schema.nodeFromJSON({
      type: "embed",
      attrs: { id: "reference-block", entity: "task", ref: "task-target" },
    });
    const resolved = JSON.stringify(
      embedPreviewSpec(embedded, {
        state: "resolved",
        snapshot: { label: "한글 동료 업무", icon: "", status: "진행 중" },
      }),
    );
    assert.ok(resolved.includes("한글 동료 업무"));
    assert.ok(resolved.includes("task-target"));
    assert.ok(resolved.includes("reference-block"));
    assert.ok(resolved.includes("진행 중"));
    assert.ok(
      JSON.stringify(embedPreviewSpec(embedded, { state: "inaccessible" })).includes("task-target"),
    );
  } finally {
    local.close();
    ydoc.destroy();
  }
});

function previewCssWrites(preview: ReturnType<typeof sanitizeEditorModePreview>) {
  const writes: { tag: string; property: string; value: string }[] = [];
  const targets = new Map<
    string,
    {
      tagName: string;
      style: { setProperty(property: string, value: string): void };
      removeAttribute(name: string): void;
    }
  >();
  for (const match of preview.html.matchAll(
    /<([a-z][a-z0-9]*)\b[^>]*data-fvoci-preview-css="(\d+)"[^>]*>/g,
  )) {
    const tag = match[1],
      marker = match[2];
    assert.ok(tag && marker);
    targets.set(marker, {
      tagName: tag.toUpperCase(),
      style: {
        setProperty(property, value) {
          writes.push({ tag, property, value });
        },
      },
      removeAttribute() {},
    });
  }
  // Controlled native DOM boundary, not a DOM parser/CSP or browser substitute.
  const root = {
    querySelector(selector: string) {
      const marker = /="(\d+)"/.exec(selector)?.[1];
      return marker === undefined ? null : (targets.get(marker) ?? null);
    },
  } as unknown as HTMLElement;
  applyEditorModePreviewStyles(root, preview);
  return writes;
}

await test("preview keeps supported heading/callout/table geometry and moves validated presentation to CSSOM before the HTML sink", () => {
  const preview = sanitizeEditorModePreview(
    '<h4>Level four</h4><h5>Level five</h5><h6>Level six</h6><aside class="afn-callout" data-callout="" data-kind="warning"><p>주의</p></aside><p style="text-align:right"><span style="color:#112233;background-color:#abcdef">색상</span></p><table style="width:300px"><colgroup><col style="width:120px"><col style="width:180px"></colgroup><tbody><tr><td colspan="2" rowspan="2" style="background:#abcdef"><p>셀</p></td></tr></tbody></table><span style="color:expression(evil());background:url(javascript:evil());text-align:evil()" onclick="evil()">bad</span>',
  );
  const html = preview.html;
  assert.ok(html.includes("<h4>Level four</h4>"));
  assert.ok(html.includes("<h5>Level five</h5>"));
  assert.ok(html.includes("<h6>Level six</h6>"));
  assert.ok(html.includes("<aside") && html.includes('data-kind="warning"'));
  assert.ok(html.includes("<colgroup>"));
  assert.ok(html.includes('colspan="2"') && html.includes('rowspan="2"'));
  assert.equal(/\sstyle=/.test(html), false);
  assert.equal(html.includes("expression"), false);
  assert.equal(html.includes("javascript:"), false);
  assert.equal(html.includes("onclick"), false);
  assert.deepEqual(previewCssWrites(preview), [
    { tag: "p", property: "text-align", value: "right" },
    { tag: "span", property: "color", value: "#112233" },
    { tag: "span", property: "background-color", value: "#abcdef" },
    { tag: "table", property: "width", value: "300px" },
    { tag: "col", property: "width", value: "120px" },
    { tag: "col", property: "width", value: "180px" },
    { tag: "td", property: "background", value: "#abcdef" },
  ]);
});

await test("preview canonical style sidecar cannot consume quoted text, entity attribute injection or stored private markers", () => {
  const preview = sanitizeEditorModePreview(
    '<p title="&quot; > style=&quot;color:red&quot;">한글 literal style="color:url(javascript:literal)" &lt;span style="color:red"&gt;</p><span data-fvoci-preview-css="0" style="color:&#35;112233;background-color:rgba(0, 1, 2, 0.5)" title="x&quot; style=&quot;position:absolute">색상</span><h4 data-fvoci-preview-css="0" style="text-align:right;position:absolute;left:-999px">제목</h4><span style="color:var(--attacker);background-color:url(https://attacker.invalid/a);position:fixed" onerror="bad()">거부</span>',
  );
  assert.ok(preview.html.includes('literal style="color:url(javascript:literal)"'));
  assert.ok(preview.html.includes('&lt;span style="color:red"&gt;'));
  assert.equal(preview.html.includes("title="), false);
  assert.equal(preview.html.includes("onerror="), false);
  assert.equal(preview.html.includes("attacker.invalid"), false);
  assert.deepEqual(previewCssWrites(preview), [
    { tag: "span", property: "color", value: "#112233" },
    { tag: "span", property: "background-color", value: "rgba(0, 1, 2, 0.5)" },
    { tag: "h4", property: "text-align", value: "right" },
  ]);
});

await test("preview CSSOM requires the producer's exact sidecar owner and a matching target tag", () => {
  const preview = sanitizeEditorModePreview('<p style="text-align:right">한글</p>');
  let writes = 0;
  const root = {
    querySelector() {
      return {
        tagName: "DIV",
        style: {
          setProperty() {
            writes++;
          },
        },
      };
    },
  } as unknown as HTMLElement;
  applyEditorModePreviewStyles(root, preview);
  assert.equal(writes, 0);
  assert.throws(() => {
    applyEditorModePreviewStyles(root, { html: preview.html });
  }, /Unowned preview presentation/);
  assert.equal(writes, 0);
});

await test("source versus ordinary rich splitBlock peer-boundary undo control records exact CRDT ordering and anchors", () => {
  for (const mode of ["source", "rich"]) {
    const initial = {
      type: "doc" as const,
      content: [
        {
          type: "paragraph",
          attrs: { id: "split", textAlign: "right" },
          content: [
            {
              type: "text",
              text: "alpha beta",
              marks: [{ type: "underline" }, { type: "textStyle", attrs: { color: "#112233" } }],
            },
          ],
        },
      ],
    };
    const localDoc = tiptapJsonToYDoc(initial),
      peerDoc = new Y.Doc({ gc: false });
    Y.applyUpdate(peerDoc, Y.encodeStateAsUpdate(localDoc));
    const local = liveEditor(localDoc),
      peer = liveEditor(peerDoc),
      wire = Symbol("wire");
    localDoc.on("update", (u, o) => {
      if (o !== wire) Y.applyUpdate(peerDoc, u, wire);
    });
    peerDoc.on("update", (u, o) => {
      if (o !== wire) Y.applyUpdate(localDoc, u, wire);
    });
    const source = new SourceModeSession(
      localDoc,
      () => 1,
      () => true,
    );
    let steps: unknown[] = [];
    if (mode === "source") {
      const cap = source.capture(local.editor.state.doc);
      const p = source.prepare(cap, "alpha\n\nbeta", local.editor.state);
      assert.equal(p.status, "ready");
      assert.ok(p.transaction);
      steps = p.transaction.steps.map((s): unknown => s.toJSON());
      source.apply(p, local.editor);
    } else {
      local.host.dispatch(
        local.editor.state.tr.setSelection(TextSelection.create(local.editor.state.doc, 6, 7)),
      );
      local.manager.stopCapturing();
      assert.equal(
        splitBlock(local.editor.state, (tr) => {
          steps = tr.steps.map((s): unknown => s.toJSON());
          local.host.dispatch(tr);
        }),
        true,
      );
      local.manager.stopCapturing();
    }
    const afterSplit: unknown = local.editor.state.doc.toJSON();
    peer.host.dispatch(peer.editor.state.tr.insertText(" peer", 6));
    const text = localDoc.getXmlFragment("prosemirror").get(0) as Y.XmlElement;
    const child = text.get(0) as Y.XmlText;
    const anchor = Y.createRelativePositionFromTypeIndex(child, 5, 0);
    const afterPeer: unknown = local.editor.state.doc.toJSON();
    local.manager.undo();
    const afterUndo: unknown = local.editor.state.doc.toJSON();
    const absolute = Y.createAbsolutePositionFromRelativePosition(anchor, localDoc);
    console.log(
      JSON.stringify({
        mode,
        steps,
        afterSplit,
        afterPeer,
        afterUndo,
        anchor,
        absolute: absolute && {
          index: absolute.index,
          type: (absolute.type as Y.XmlText).toString() as unknown,
        },
        peerAfterUndo: peer.editor.state.doc.toJSON() as unknown,
        rawLocal: yDocToTiptapJson(localDoc),
        rawPeer: yDocToTiptapJson(peerDoc),
        converged: Y.equalSnapshots(Y.snapshot(localDoc), Y.snapshot(peerDoc)),
        undoItems: local.manager.undoStack.length,
      }),
    );
    assert.equal(local.editor.state.doc.textContent.includes(" peer"), true);
    assert.equal(local.editor.state.doc.textContent, peer.editor.state.doc.textContent);
    assert.deepEqual(Y.encodeStateAsUpdate(localDoc), Y.encodeStateAsUpdate(peerDoc));
    assert.equal(local.editor.state.doc.child(0).attrs.id, "split");
    source.destroy();
    local.close();
    peer.close();
    localDoc.destroy();
    peerDoc.destroy();
  }
});
