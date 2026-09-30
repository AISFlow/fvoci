import assert from "node:assert/strict";
import test from "node:test";
import { Editor } from "@tiptap/core";
import { DecorationSet } from "@tiptap/pm/view";
import * as Y from "yjs";
import { createFvociEditorExtensions } from "../src/editor-extensions.ts";
import { lowlight } from "../src/lowlight.ts";
import { createFvociExtensions } from "../src/tiptap-schema.ts";

await test("a rejected mention lookup reports its error and retains the stored label", async () => {
  const failure = new Error("mention lookup failed");
  const reports: unknown[][] = [];
  const originalError = console.error;
  const originalDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const span = { textContent: "", setAttribute() {} };
  const ydoc = new Y.Doc();
  const stubView = () => () => ({ dom: {} as HTMLElement });
  const editor = new Editor({
    element: null,
    content: { type: "doc", content: [{ type: "paragraph" }] },
    extensions: createFvociEditorExtensions({
      ydoc,
      nodeViews: { math: stubView, mathInline: stubView, embed: stubView, attachment: stubView },
      mentionItems: () => undefined,
      entityResolver: () => () => Promise.reject(failure),
      workspaceSlug: () => null,
      uploads: { anchors: new Map(), queue: () => {} },
    }),
  });
  try {
    const reported = new Promise<void>((resolve) => {
      console.error = (...args: unknown[]) => {
        reports.push(args);
        resolve();
      };
    });
    Object.defineProperty(globalThis, "document", {
      configurable: true,
      value: { createElement: () => span },
    });
    const constructor = editor.extensionManager.nodeViews.mention;
    const type = editor.schema.nodes.mention;
    assert.ok(constructor && type);
    constructor(
      type.create({ entity: "user", id: "user-id", label: "Stored" }),
      editor.view,
      () => 0,
      [],
      DecorationSet.empty,
    );
    await reported;
    assert.equal(span.textContent, "@Stored");
    assert.deepEqual(reports, [["Failed to resolve editor mention", failure]]);
    assert.equal(editor.state.doc.textContent, "");
  } finally {
    console.error = originalError;
    if (originalDocument) Object.defineProperty(globalThis, "document", originalDocument);
    else Reflect.deleteProperty(globalThis, "document");
    editor.destroy();
    ydoc.destroy();
  }
});

await test("a rejected lazy grammar registration reports its error without changing the document", async () => {
  const failure = new Error("grammar registration failed");
  const reports: unknown[][] = [];
  const originalError = console.error;
  const register = lowlight.register;
  const editor = new Editor({
    element: null,
    extensions: createFvociExtensions(),
    content: {
      type: "doc",
      content: [
        {
          type: "codeBlock",
          attrs: { language: "python" },
          content: [{ type: "text", text: "print(1)" }],
        },
      ],
    },
  });
  try {
    const reported = new Promise<void>((resolve) => {
      console.error = (...args: unknown[]) => {
        reports.push(args);
        resolve();
      };
    });
    lowlight.register = () => {
      throw failure;
    };
    editor.commands.insertContentAt(2, { type: "text", text: "x" });
    const written = editor.getJSON();
    await reported;
    assert.deepEqual(reports, [
      ["Failed to refresh editor code-block language", "python", failure],
    ]);
    assert.deepEqual(editor.getJSON(), written);
    assert.equal(editor.state.doc.textContent, "pxrint(1)");
  } finally {
    console.error = originalError;
    lowlight.register = register;
    editor.destroy();
  }
});
