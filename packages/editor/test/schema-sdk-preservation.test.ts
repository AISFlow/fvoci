import assert from "node:assert/strict";
import test from "node:test";
import { Editor } from "@tiptap/core";
import { EditorState, TextSelection } from "@tiptap/pm/state";
import { createFvociExtensions } from "../src/tiptap-schema.ts";

// SDK plugin tests use a headless Editor. Actual DOM/provider/persistence
// behavior is exercised separately by main-alignment-schema-roundtrip.spec.ts.
await test("marked Unicode stays text so its marks survive the CRDT representation", () => {
  const editor = new Editor({
    element: null,
    extensions: createFvociExtensions(),
    content: { type: "doc", content: [{ type: "paragraph" }] },
  });
  try {
    const paragraph = editor.schema.nodeFromJSON({
      type: "doc",
      content: [
        {
          type: "paragraph",
          attrs: { id: "p" },
          content: [
            { type: "text", text: "한글 😀 🧑‍💻", marks: [{ type: "bold" }, { type: "italic" }] },
          ],
        },
      ],
    });
    const before = EditorState.create({
      schema: editor.schema,
      doc: paragraph,
      plugins: editor.extensionManager.plugins,
    });
    const transaction = before.tr.insertText("!", 1);
    const after = before.apply(transaction);
    let emojis = 0;
    after.doc.descendants((node) => {
      if (node.type.name !== "emoji") return;
      emojis++;
      assert.deepEqual(
        node.marks.map((mark) => mark.type.name),
        ["bold", "italic"],
      );
    });
    assert.equal(emojis, 0);
    assert.equal(after.doc.firstChild?.textContent, "!한글 😀 🧑‍💻");
    assert.deepEqual(
      after.doc.firstChild.lastChild?.marks.map((mark) => mark.type.name),
      ["bold", "italic"],
    );
  } finally {
    editor.destroy();
  }
});

await test("heading initialization keeps an existing block ID when its TOC anchor is absent", () => {
  const editor = new Editor({
    element: null,
    extensions: createFvociExtensions(),
    content: { type: "doc", content: [{ type: "paragraph" }] },
  });
  const priorWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  try {
    const doc = editor.schema.nodeFromJSON({
      type: "doc",
      content: [
        {
          type: "heading",
          attrs: { id: "stable-ref", level: 2 },
          content: [{ type: "text", text: "한글 제목" }],
        },
      ],
    });
    const state = EditorState.create({
      schema: editor.schema,
      doc,
      plugins: editor.extensionManager.plugins,
    });
    // The installed TOC plugin has an explicit SSR guard; enable that branch
    // without claiming this is a real browser/DOM witness.
    Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
    const after = state.apply(state.tr.insertText("!", 1));
    assert.equal(after.doc.firstChild?.attrs.id, "stable-ref");
    assert.equal(after.doc.firstChild.attrs["data-toc-id"], "stable-ref");
  } finally {
    if (priorWindow) Object.defineProperty(globalThis, "window", priorWindow);
    else Reflect.deleteProperty(globalThis, "window");
    editor.destroy();
  }
});

await test("heading missing IDs, duplicate identities and colliding anchors stay distinct", () => {
  const editor = new Editor({
    element: null,
    extensions: createFvociExtensions(),
    content: { type: "doc", content: [{ type: "paragraph" }] },
  });
  const priorWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  try {
    const attrs = [
      { id: "stable", "data-toc-id": "occupied" },
      { id: "stable" },
      { id: "occupied" },
      {},
      { "data-toc-id": "existing-anchor" },
      { id: "other", "data-toc-id": "occupied" },
    ];
    const doc = editor.schema.nodeFromJSON({
      type: "doc",
      content: attrs.map((a) => ({
        type: "heading",
        attrs: a,
        content: [{ type: "text", text: "제목" }],
      })),
    });
    const before = EditorState.create({
      schema: editor.schema,
      doc,
      plugins: editor.extensionManager.plugins,
    });
    Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
    const after = before.apply(before.tr.insertText("!", 1));
    const ids: unknown[] = [],
      anchors: unknown[] = [];
    after.doc.forEach((node) => {
      if (node.type.name === "heading") {
        ids.push(node.attrs.id);
        anchors.push(node.attrs["data-toc-id"]);
      }
    });
    assert.equal(ids[0], "stable");
    assert.equal(ids[2], "occupied");
    assert.equal(ids[4], "existing-anchor");
    assert.equal(ids[5], "other");
    assert.equal(new Set(ids).size, 6);
    assert.equal(new Set(anchors).size, 6);
    assert.ok(ids.every((id) => typeof id === "string" && id.length > 0));
    assert.equal(after.apply(after.tr.insertText("!", 1)).doc.child(2).attrs.id, "occupied");
  } finally {
    if (priorWindow) Object.defineProperty(globalThis, "window", priorWindow);
    else Reflect.deleteProperty(globalThis, "window");
    editor.destroy();
  }
});

await test("mixed-mark emoji conversion keeps selection and explicit stored marks", () => {
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
          attrs: { id: "mixed" },
          content: [
            { type: "text", text: "😀", marks: [{ type: "bold" }] },
            {
              type: "text",
              text: " 🧑‍💻",
              marks: [
                { type: "italic" },
                { type: "link", attrs: { href: "https://example.com/" } },
              ],
            },
            { type: "text", text: " 끝 😀" },
          ],
        },
      ],
    });
    const before = EditorState.create({
      schema: editor.schema,
      plugins: editor.extensionManager.plugins,
    });
    const tr = before.tr.replaceWith(0, before.doc.content.size, doc.content);
    tr.setSelection(TextSelection.create(tr.doc, tr.doc.content.size - 1));
    const after = before.apply(tr.setStoredMarks([editor.schema.mark("underline")]));
    const marks: string[][] = [];
    after.doc.descendants((node) => {
      if (node.type.name === "emoji") marks.push(node.marks.map((mark) => mark.type.name));
    });
    assert.deepEqual(marks, [[]], "unmarked Unicode still becomes an emoji atom");
    const markedText: { text: string; marks: string[] }[] = [];
    after.doc.descendants((node) => {
      if (node.isText && node.marks.length)
        markedText.push({ text: node.text ?? "", marks: node.marks.map((mark) => mark.type.name) });
    });
    assert.deepEqual(markedText, [
      { text: "😀", marks: ["bold"] },
      { text: " 🧑‍💻", marks: ["link", "italic"] },
    ]);
    assert.equal(after.selection.$from.parent.attrs.id, "mixed");
    assert.equal(after.selection.$from.parentOffset, after.selection.$from.parent.content.size);
    assert.deepEqual(
      after.storedMarks?.map((mark) => mark.type.name),
      ["underline"],
    );
  } finally {
    editor.destroy();
  }
});
