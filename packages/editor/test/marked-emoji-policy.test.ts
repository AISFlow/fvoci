import assert from "node:assert/strict";
import test from "node:test";
import { Editor } from "@tiptap/core";
import { EditorState, TextSelection } from "@tiptap/pm/state";
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
