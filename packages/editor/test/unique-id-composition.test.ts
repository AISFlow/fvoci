import assert from "node:assert/strict";
import test from "node:test";
import { Editor, getExtensionField, type AnyConfig, type Extension } from "@tiptap/core";
import { HocuspocusProvider, HocuspocusProviderWebsocket } from "@hocuspocus/provider";
import type { UniqueIDOptions } from "@tiptap/extension-unique-id";
import { EditorState, TextSelection, type PluginView } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import { ySyncPluginKey } from "@tiptap/y-tiptap";
import { Awareness } from "y-protocols/awareness";
import * as Y from "yjs";
import { createFvociEditorExtensions, type FvociNodeViews } from "../src/editor-extensions.ts";

// #258: UniqueID must not give a block its id in the transaction of an IME
// composition update. The browser tests are the CDP composition test and the
// opt-in IBus witness in apps/web/e2e-pending/workspace-wiki-ime.spec.ts;
// this one pins the transaction rule on the editor's real extension list.

const stubView = () => () => ({ dom: {} as HTMLElement });
const nodeViews: FvociNodeViews = {
  mermaid: stubView,
  math: stubView,
  mathInline: stubView,
  embed: stubView,
  attachment: stubView,
};

/** The collaborative editor's plugins over a document whose blocks have no
 * ids, as the Rust seed stores a body without them. The provider comes with
 * peer carets, as in the product, where the editor mounts after the provider
 * synced. Subsequent sync events must also leave viewing identity alone;
 * ids belong to an actual local non-composition edit. */
function withPlugins(texts: string[], ids: (string | null)[] = []) {
  const ydoc = new Y.Doc({ gc: false });
  const provider = { awareness: new Awareness(ydoc), on() {}, off() {} };
  const editor = new Editor({
    element: null,
    extensions: createFvociEditorExtensions({
      ydoc,
      nodeViews,
      mentionItems: () => undefined,
      entityResolver: () => null,
      workspaceSlug: () => null,
      uploads: { anchors: new Map(), queue: () => {} },
      provider: provider as unknown as NonNullable<
        Parameters<typeof createFvociEditorExtensions>[0]["provider"]
      >,
      user: { id: "u1", name: "A", color: "#336699" },
    }),
    // Headless: a JSON doc, since the default "" content is parsed as HTML.
    content: { type: "doc", content: [{ type: "paragraph" }] },
  });
  const { schema } = editor;
  const doc = schema.nodeFromJSON({
    type: "doc",
    content: texts.map((text, i) => ({
      type: "paragraph",
      attrs: { id: ids[i] ?? null },
      ...(text ? { content: [{ type: "text", text }] } : {}),
    })),
  });
  const state = EditorState.create({ schema, doc, plugins: editor.extensionManager.plugins });
  editor.destroy();
  provider.awareness.destroy();
  return state;
}

const idsOf = (state: EditorState) => {
  const ids: unknown[] = [];
  state.doc.forEach((node) => ids.push(node.attrs.id));
  return ids;
};

await test("a composition update leaves a block without an id alone; the next plain edit gives it one", () => {
  // "첫 문단" is positions 1..5; the IME's first update inserts the jamo at its end.
  let state = withPlugins(["첫 문단", ""]);
  state = state.apply(state.tr.insertText("ㅎ", 5).setMeta("composition", 1));
  assert.deepEqual(idsOf(state), [null, null]);
  state = state.apply(state.tr.insertText("하", 5, 6).setMeta("composition", 1));
  assert.equal(state.doc.child(0).textContent, "첫 문단하");
  assert.deepEqual(idsOf(state), [null, null]);
  // The commit (Chromium's insertText after compositionend) carries no composition meta.
  state = state.apply(state.tr.insertText(" ", 6));
  const [first, second] = idsOf(state);
  assert.equal(typeof first, "string");
  assert.equal(second, null);
});

await test("a composition in an empty block without an id leaves it alone too", () => {
  let state = withPlugins(["첫 문단", ""]);
  // The empty paragraph opens at 6, so its content starts at 7.
  state = state.apply(state.tr.insertText("ㅎ", 7).setMeta("composition", 1));
  assert.equal(state.doc.child(1).textContent, "ㅎ");
  assert.deepEqual(idsOf(state), [null, null]);
});

await test("a plain local edit still gives the block an id in the same dispatch", () => {
  let state = withPlugins(["첫 문단", ""]);
  state = state.apply(state.tr.insertText("x", 5));
  const [first, second] = idsOf(state);
  assert.equal(typeof first, "string");
  assert.equal(second, null);
});

await test("a Yjs-origin change still gives no id", () => {
  let state = withPlugins(["첫 문단"]);
  state = state.apply(
    state.tr.insertText("원격", 5).setMeta(ySyncPluginKey, { isChangeOrigin: true }),
  );
  assert.deepEqual(idsOf(state), [null]);
});

await test("splitting a block outside a composition still leaves two distinct ids", () => {
  let state = withPlugins(["첫 문단"], ["block-a"]);
  const tr = state.tr.setSelection(TextSelection.create(state.doc, 2)).split(2);
  state = state.apply(tr);
  const ids = idsOf(state);
  assert.equal(ids.length, 2);
  assert.ok(ids.includes("block-a"));
  assert.equal(new Set(ids).size, 2);
  assert.ok(ids.every((id) => typeof id === "string"));
});

/** Real SDK lifecycle, PM plugins and Y binding, without a DOM editor mount.
 * The only view carrier addition is hasFocus=false. The installed binding
 * receives every actual Tiptap dispatch; provider.synced uses the SDK's public
 * setter/event ordering. No socket connects, and browser/DB proof is separate. */
function lifecycle(editable: boolean, withProvider = true) {
  const ydoc = new Y.Doc({ gc: false });
  let allocations = 0;
  const socket = new HocuspocusProviderWebsocket({ url: "ws://127.0.0.1:1", autoConnect: false });
  assert.equal(socket.configuration.autoConnect, false);
  assert.equal(socket.webSocket, null);
  const provider = new HocuspocusProvider({
    websocketProvider: socket,
    name: "identity-lifecycle",
    document: ydoc,
  });
  provider.synced = true;
  const editor = new Editor({
    element: null,
    editable,
    extensions: createFvociEditorExtensions({
      ydoc,
      nodeViews,
      mentionItems: () => undefined,
      entityResolver: () => null,
      workspaceSlug: () => null,
      uploads: { anchors: new Map(), queue: () => {} },
      ...(withProvider ? { provider, user: { id: "u1", name: "A", color: "#336699" } } : {}),
    }).map((extension) => {
      if (extension.name !== "uniqueID") return extension;
      const unique = extension as Extension<UniqueIDOptions>;
      const generate = unique.options.generateID;
      return unique.configure({
        generateID: (...args: Parameters<UniqueIDOptions["generateID"]>) => {
          allocations++;
          const id: unknown = generate(...args);
          assert.ok(typeof id === "string");
          return id;
        },
      });
    }),
    content: { type: "doc", content: [{ type: "paragraph" }] },
  });
  editor.view.updateState(
    EditorState.create({
      schema: editor.schema,
      doc: editor.state.doc,
      plugins: editor.extensionManager.plugins,
    }),
  );
  const view: EditorView = new Proxy(editor.view, {
    get(target, key) {
      return key === "hasFocus" ? () => false : (Reflect.get(target, key) as unknown);
    },
  });
  let previous = editor.state;
  const syncView: { current: PluginView | undefined } = { current: undefined };
  let dispatches = 0;
  let updates = 0;
  let localUpdates = 0;
  const transactions = () => {
    dispatches++;
    const before = previous;
    previous = editor.state;
    syncView.current?.update?.(view, before);
  };
  editor.on("transaction", transactions);
  ydoc.on("update", (_bytes, _origin, _doc, transaction) => {
    updates++;
    if (transaction.local) localUpdates++;
  });
  const sync = editor.state.plugins.find((plugin) => plugin.spec.key === ySyncPluginKey);
  assert.ok(sync?.spec.view);
  syncView.current = sync.spec.view(view);
  const unique = editor.extensionManager.extensions.find(
    (extension) => extension.name === "uniqueID",
  ) as Extension<UniqueIDOptions, Record<string, unknown>> | undefined;
  assert.ok(unique);
  const create = getExtensionField<AnyConfig["onCreate"]>(unique, "onCreate", {
    name: unique.name,
    options: unique.options,
    storage: unique.storage,
    editor,
    type: null,
  });
  return {
    editor,
    provider,
    ydoc,
    create: () => create?.({ editor }),
    counts: () => ({ allocations, updates, localUpdates, dispatches }),
    close() {
      editor.off("transaction", transactions);
      syncView.current?.destroy?.();
      editor.destroy();
      provider.destroy();
      socket.destroy();
      ydoc.destroy();
    },
  };
}

for (const editable of [false, true]) {
  await test(`actual UniqueID create/re-sync does not allocate or publish while viewing editable=${String(editable)}`, () => {
    const fixture = lifecycle(editable);
    try {
      const before = fixture.editor.state.doc;
      const bytes = Y.encodeStateAsUpdate(fixture.ydoc);
      const counts = fixture.counts();
      fixture.create();
      fixture.provider.synced = false;
      fixture.provider.synced = true;
      fixture.provider.synced = false;
      fixture.provider.synced = true;
      assert.equal(fixture.editor.isEditable, editable);
      assert.deepEqual(
        fixture.counts(),
        counts,
        "no UUID, PM dispatch or Y publication on view/re-sync",
      );
      assert.ok(fixture.editor.state.doc.eq(before), "viewing cannot assign paragraph identity");
      assert.deepEqual(Y.encodeStateAsUpdate(fixture.ydoc), bytes);
      assert.equal(fixture.ydoc.getXmlFragment("prosemirror").toJSON(), "");
      fixture.editor.setEditable(true);
      const afterGrant = fixture.counts();
      fixture.editor.view.dispatch(fixture.editor.state.tr.insertText("한글"));
      assert.equal(fixture.editor.state.doc.textContent, "한글");
      const id: unknown = fixture.editor.state.doc.firstChild?.attrs.id;
      assert.ok(typeof id === "string");
      assert.match(id, /^[a-f0-9-]{36}$/);
      assert.equal(fixture.counts().allocations - afterGrant.allocations, 1);
      assert.equal(fixture.counts().updates - afterGrant.updates, 1);
      assert.match(fixture.ydoc.getXmlFragment("prosemirror").toJSON(), /한글/);
    } finally {
      fixture.close();
    }
  });
}

await test("actual peer sync keeps its native paragraph ID/text without local maintenance writes", () => {
  const fixture = lifecycle(false);
  const peer = new Y.Doc({ gc: false });
  try {
    fixture.create();
    const paragraph = new Y.XmlElement("paragraph");
    const text = new Y.XmlText();
    peer.getXmlFragment("prosemirror").insert(0, [paragraph]);
    paragraph.setAttribute("id", "peer-stable");
    paragraph.insert(0, [text]);
    text.insert(0, "동료 🧑‍💻");
    const before = fixture.counts();
    Y.applyUpdate(fixture.ydoc, Y.encodeStateAsUpdate(peer), fixture.provider);
    fixture.provider.synced = false;
    fixture.provider.synced = true;
    assert.equal(
      fixture.editor.getText(),
      "동료 🧑‍💻",
      JSON.stringify({
        pm: fixture.editor.getJSON(),
        native: fixture.ydoc.getXmlFragment("prosemirror").toJSON(),
        counts: fixture.counts(),
      }),
    );
    const paragraphNode = fixture.editor.state.doc.firstChild;
    assert.ok(paragraphNode);
    const emoji = paragraphNode.lastChild;
    assert.ok(emoji);
    assert.equal(emoji.type.name, "emoji");
    assert.equal(emoji.attrs.name, "technologist");
    assert.equal(paragraphNode.attrs.id, "peer-stable");
    assert.equal(fixture.counts().allocations, before.allocations);
    assert.equal(fixture.counts().localUpdates, before.localUpdates);
    assert.deepEqual(Y.encodeStateVector(fixture.ydoc), Y.encodeStateVector(peer));
    assert.equal(
      fixture.ydoc.getXmlFragment("prosemirror").toJSON(),
      '<paragraph id="peer-stable">동료 🧑‍💻</paragraph>',
    );
  } finally {
    fixture.close();
    peer.destroy();
  }
});

await test("re-sync during real composition allocates nothing; actual plain commit assigns one ID", () => {
  const fixture = lifecycle(true);
  try {
    fixture.create();
    fixture.editor.view.dispatch(
      fixture.editor.state.tr.insertText("ㅎ").setMeta("composition", 1),
    );
    const composed = fixture.editor.state.doc.firstChild;
    assert.ok(composed);
    assert.equal(composed.attrs.id, null);
    const before = fixture.counts();
    const bytes = Y.encodeStateAsUpdate(fixture.ydoc);
    fixture.provider.synced = false;
    fixture.provider.synced = true;
    assert.deepEqual(fixture.counts(), before);
    assert.deepEqual(Y.encodeStateAsUpdate(fixture.ydoc), bytes);
    const afterSync = fixture.editor.state.doc.firstChild;
    assert.ok(afterSync);
    assert.equal(afterSync.attrs.id, null);
    fixture.editor.view.dispatch(fixture.editor.state.tr.insertText("한", 1, 2));
    assert.equal(fixture.editor.state.doc.textContent, "한");
    const id: unknown = fixture.editor.state.doc.firstChild?.attrs.id;
    assert.ok(typeof id === "string");
    assert.match(id, /^[a-f0-9-]{36}$/);
    assert.equal(fixture.counts().allocations - before.allocations, 1);
    assert.equal(fixture.counts().localUpdates - before.localUpdates, 1);
  } finally {
    fixture.close();
  }
});

await test("no-provider factory retains SDK initial projection identity rather than a later synced listener", () => {
  const fixture = lifecycle(false, false);
  try {
    // The product Vue shell requires provider+user. This separate headless
    // factory path retains SDK needsInitialIdGeneration during initial y-sync;
    // it is not evidence of zero-ID readonly viewing for a provider-less host.
    const id: unknown = fixture.editor.state.doc.firstChild?.attrs.id;
    assert.ok(typeof id === "string");
    assert.match(id, /^[a-f0-9-]{36}$/);
    const before = fixture.counts();
    fixture.create();
    fixture.provider.synced = false;
    fixture.provider.synced = true;
    assert.deepEqual(fixture.counts(), before);
    fixture.editor.setEditable(true);
    fixture.editor.view.dispatch(fixture.editor.state.tr.insertText("offline"));
    assert.equal(fixture.editor.state.doc.firstChild?.attrs.id, id);
    assert.equal(fixture.editor.state.doc.textContent, "offline");
  } finally {
    fixture.close();
  }
});
