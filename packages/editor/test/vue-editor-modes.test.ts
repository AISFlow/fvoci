import assert from "node:assert/strict";
import test from "node:test";
import { Editor } from "@tiptap/core";
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
import { sanitizeEditorModePreview } from "../src/vue/editor-mode-preview.ts";

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
  );
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
