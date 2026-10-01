import assert from "node:assert/strict";
import test from "node:test";
import { Editor } from "@tiptap/core";
import { EditorState, TextSelection, type Plugin } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import { initProseMirrorDoc, ySyncPlugin, ySyncPluginKey } from "@tiptap/y-tiptap";
import * as Y from "yjs";
import { replaceYDocContent, tiptapJsonToYDoc, yDocToTiptapJson } from "../src/collab-tiptap.ts";
import { createFvociExtensions } from "../src/tiptap-schema.ts";
import type { TiptapDoc } from "../src/json.ts";

await test("unrepresentable marked emoji import fails before changing existing Y.Doc", () => {
  const doc = tiptapJsonToYDoc({
    type: "doc",
    content: [{ type: "paragraph", content: [{ type: "text", text: "원본" }] }],
  });
  const before = Y.encodeStateAsUpdate(doc);
  for (const name of ["grinning", "unknown-custom"]) {
    const invalid: TiptapDoc = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [{ type: "emoji", attrs: { name }, marks: [{ type: "bold" }] }],
        },
      ],
    };
    assert.throws(() => {
      tiptapJsonToYDoc(invalid);
    }, /Marked emoji/);
    assert.throws(() => {
      replaceYDocContent(doc, invalid);
    }, /Marked emoji/);
    assert.deepEqual(Y.encodeStateAsUpdate(doc), before);
  }
  doc.destroy();
});

await test("marking an existing emoji atom becomes Unicode text with the same marks and cursor; fresh CRDT keeps it", () => {
  const editor = new Editor({
    element: null,
    extensions: createFvociExtensions(),
    content: { type: "doc", content: [{ type: "paragraph" }] },
  });
  try {
    const doc = editor.schema.nodeFromJSON({
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "atom" },
          content: [{ type: "emoji", attrs: { name: "grinning" } }],
        },
      ],
    });
    const before = EditorState.create({
      schema: editor.schema,
      doc,
      plugins: editor.extensionManager.plugins,
      selection: TextSelection.create(doc, 2),
    });
    const after = before.apply(before.tr.addMark(1, 2, editor.schema.mark("bold")));
    assert.equal(after.doc.firstChild?.firstChild?.type.name, "text");
    assert.equal(after.doc.firstChild.textContent, "😀");
    assert.deepEqual(
      after.doc.firstChild.firstChild.marks.map((m) => m.type.name),
      ["bold"],
    );
    assert.equal(after.selection.from, 3);
    const encoded = tiptapJsonToYDoc(after.doc.toJSON() as TiptapDoc);
    const fresh = new Y.Doc({ gc: false });
    Y.applyUpdate(fresh, Y.encodeStateAsUpdate(encoded));
    assert.ok(editor.schema.nodeFromJSON(yDocToTiptapJson(fresh)).eq(after.doc));
    encoded.destroy();
    fresh.destroy();
  } finally {
    editor.destroy();
  }
});

await test("a custom emoji with no SDK glyph refuses a mark transaction without mutation", () => {
  const editor = new Editor({
    element: null,
    extensions: createFvociExtensions(),
    content: { type: "doc", content: [{ type: "paragraph" }] },
  });
  try {
    const doc = editor.schema.nodeFromJSON({
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "emoji", attrs: { name: "unknown-custom" } }] },
      ],
    });
    const before = EditorState.create({
      schema: editor.schema,
      doc,
      plugins: editor.extensionManager.plugins,
    });
    const result = before.applyTransaction(before.tr.addMark(1, 2, editor.schema.mark("bold")));
    assert.equal(result.transactions.length, 0);
    assert.ok(result.state.doc.eq(before.doc));
  } finally {
    editor.destroy();
  }
});

await test("live ySync refuses composing atom marks, then publishes the ordinary command with selection and undo intact", () => {
  const editor = new Editor({
    element: null,
    extensions: createFvociExtensions(),
    content: { type: "doc", content: [{ type: "paragraph" }] },
  });
  const controlledView = { composing: true };
  Object.defineProperty(editor, "view", { configurable: true, value: controlledView });
  const doc = tiptapJsonToYDoc({
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: "live" },
        content: [
          { type: "emoji", attrs: { name: "grinning" } },
          { type: "text", text: " 한글" },
        ],
      },
    ],
  });
  const fragment = doc.getXmlFragment("prosemirror");
  const init = initProseMirrorDoc(fragment, editor.schema);
  // The installed SDK declares this return as any; its runtime is a PM plugin.
  const sync = ySyncPlugin(fragment, { mapping: init.mapping }) as unknown as Plugin;
  const original = init.doc;
  let state = EditorState.create({
    schema: editor.schema,
    doc: original,
    plugins: [sync, ...editor.extensionManager.plugins],
    selection: TextSelection.create(original, 2),
  });
  const host = {
    state,
    hasFocus: () => false,
    dispatch: (tr: import("@tiptap/pm/state").Transaction) => {
      state = state.applyTransaction(tr).state;
      host.state = state;
    },
  };
  assert.ok(sync.spec.view);
  const live = sync.spec.view(host as EditorView);
  const manager = new Y.UndoManager(fragment, { trackedOrigins: new Set([ySyncPluginKey]) });
  const updates: unknown[] = [];
  doc.on("update", () => updates.push(yDocToTiptapJson(doc)));
  try {
    const before = Y.encodeStateAsUpdate(doc);
    const refused = state.applyTransaction(state.tr.addMark(1, 2, editor.schema.mark("bold")));
    assert.equal(refused.transactions.length, 0);
    state = refused.state;
    host.state = state;
    live.update?.(host as EditorView, state);
    assert.ok(state.doc.eq(original));
    assert.equal(state.selection.from, 2);
    assert.deepEqual(Y.encodeStateAsUpdate(doc), before);
    assert.equal(updates.length, 0);
    assert.equal(manager.undoStack.length, 0);

    // This controls the SDK view flag, not physical IME/DOM behavior.
    controlledView.composing = false;
    state = state.applyTransaction(state.tr.addMark(1, 2, editor.schema.mark("bold"))).state;
    host.state = state;
    live.update?.(host as EditorView, refused.state);
    assert.equal(state.doc.firstChild?.firstChild?.type.name, "text");
    assert.equal(state.doc.firstChild.firstChild.text, "😀");
    assert.deepEqual(
      state.doc.firstChild.firstChild.marks.map((mark) => mark.type.name),
      ["bold"],
    );
    assert.equal(state.selection.from, 3);
    assert.equal(updates.length, 1);
    assert.ok(editor.schema.nodeFromJSON(yDocToTiptapJson(doc)).eq(state.doc));
    const accepted = state.doc;
    manager.stopCapturing();
    manager.undo();
    assert.ok(state.doc.eq(original));
    assert.ok(editor.schema.nodeFromJSON(yDocToTiptapJson(doc)).eq(original));
    manager.redo();
    assert.ok(state.doc.eq(accepted));
    assert.ok(editor.schema.nodeFromJSON(yDocToTiptapJson(doc)).eq(accepted));
  } finally {
    manager.destroy();
    live.destroy?.();
    doc.destroy();
    Reflect.deleteProperty(editor, "view");
    editor.destroy();
  }
});
