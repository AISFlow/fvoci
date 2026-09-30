import assert from "node:assert/strict";
import test from "node:test";
import { Editor, type JSONContent } from "@tiptap/core";
import { effectScope } from "vue";
import { moveBlock } from "../src/gutter-actions.ts";
import { HIGHLIGHT_MAX_CHARS } from "../src/lowlight.ts";
import { createFvociExtensions } from "../src/tiptap-schema.ts";
import { keyboardBlockPos } from "../src/vue/block-gutter.ts";
import { codeBlockAtCaret, codeChromeView, DEFAULT_CODE_CHROME } from "../src/vue/code-block-chrome.ts";
import { boxInHost, tableCaret } from "../src/vue/table-handles.ts";
import { sameValue, useEditorState } from "../src/vue/use-editor-state.ts";

// The Vue editing controls' composables and selectors (src/vue), on a
// headless editor with the shared schema: Tiptap applies commands to its
// state without a view, so the selectors see real documents.

const text = (value: string): JSONContent => ({ type: "text", text: value });
const paragraph = (value: string): JSONContent =>
  value ? { type: "paragraph", content: [text(value)] } : { type: "paragraph" };
const cell = (value: string): JSONContent => ({ type: "tableCell", content: [paragraph(value)] });
const table: JSONContent = {
  type: "table",
  content: [
    { type: "tableRow", content: [cell("A"), cell("B")] },
    { type: "tableRow", content: [cell("1"), cell("2")] },
  ],
};

function editorWith(content: JSONContent[]): Editor {
  return new Editor({ element: null, extensions: createFvociExtensions(), content: { type: "doc", content } });
}

/** Position just inside the first text node that reads `value`. */
function posOfText(editor: Editor, value: string): number {
  let found = -1;
  editor.state.doc.descendants((node, pos) => {
    if (found >= 0) return false;
    if (node.isText && node.text === value) found = pos;
    return true;
  });
  assert.ok(found >= 0, value);
  return found;
}

test("sameValue compares plain values by structure and anything else by identity", () => {
  assert.equal(sameValue(1, 1), true);
  assert.equal(sameValue(Number.NaN, Number.NaN), true);
  assert.equal(sameValue({ a: [1, { b: "x" }] }, { a: [1, { b: "x" }] }), true);
  assert.equal(sameValue({ a: 1 }, { a: 1, b: undefined }), false);
  assert.equal(sameValue([1, 2], [1, 2, 3]), false);
  assert.equal(sameValue(null, {}), false);
  assert.equal(sameValue({ a: 1 }, [1]), false);
  const editor = editorWith([paragraph("x")]);
  assert.equal(sameValue(editor.state.doc, editor.state.doc), true);
  assert.equal(sameValue({ node: editor.state.doc }, { node: editorWith([paragraph("x")]).state.doc }), false);
});

test("useEditorState follows transactions, keeps an equal value, and stops with its scope", () => {
  const editor = editorWith([paragraph("abc"), paragraph("def")]);
  const scope = effectScope();
  const state = scope.run(() =>
    useEditorState(editor, (current) => ({ block: current.state.selection.$from.parent.textContent })),
  );
  assert.ok(state);
  assert.deepEqual(state.value, { block: "abc" });
  const first = state.value;
  editor.commands.setTextSelection(posOfText(editor, "abc") + 2);
  assert.equal(state.value, first, "an equal value keeps the same object");
  editor.commands.setTextSelection(posOfText(editor, "def") + 1);
  assert.deepEqual(state.value, { block: "def" });
  scope.stop();
  editor.commands.setTextSelection(posOfText(editor, "abc") + 1);
  assert.deepEqual(state.value, { block: "def" }, "no update after the scope stopped");
});

test("useEditorState follows setEditable, which emits no transaction", () => {
  const editor = editorWith([paragraph("abc")]);
  const scope = effectScope();
  const editable = scope.run(() => useEditorState(editor, (current) => current.isEditable));
  assert.equal(editable?.value, true);
  editor.setEditable(false);
  assert.equal(editable?.value, false);
  scope.stop();
});

test("the keyboard gutter button acts on the caret's block and never on a table", () => {
  const editor = editorWith([
    paragraph("top"),
    table,
    { type: "bulletList", content: [{ type: "listItem", content: [paragraph("item")] }] },
  ]);
  editor.commands.setTextSelection(posOfText(editor, "top") + 1);
  assert.equal(keyboardBlockPos(editor), 0);
  editor.commands.setTextSelection(posOfText(editor, "2") + 1);
  assert.equal(keyboardBlockPos(editor), -1);
  const item = posOfText(editor, "item");
  editor.commands.setTextSelection(item + 1);
  // The innermost block: the list item's paragraph, where the gutter's
  // block menu converts, moves and deletes.
  assert.equal(keyboardBlockPos(editor), item - 1);
  assert.equal(editor.state.doc.nodeAt(item - 1)?.type.name, "paragraph");
});

test("the table caret is the table's position and the caret, or null outside tables", () => {
  const editor = editorWith([paragraph("before"), table, paragraph("after")]);
  editor.commands.setTextSelection(posOfText(editor, "before") + 1);
  assert.equal(tableCaret(editor), null);
  const inCell = posOfText(editor, "2") + 1;
  editor.commands.setTextSelection(inCell);
  assert.deepEqual(tableCaret(editor), { tablePos: editor.state.doc.child(0).nodeSize, from: inCell });
});

test("the table handles' box is the table's rectangle in the host's coordinates", () => {
  assert.deepEqual(
    boxInHost(
      { left: 130, top: 260, width: 400, height: 90 },
      { left: 100, top: 200 },
      { clientLeft: 1, clientTop: 2, scrollLeft: 5, scrollTop: 40 },
    ),
    { left: 34, top: 98, width: 400, height: 90 },
  );
});

test("the table menu's up and down move a table past its sibling block", () => {
  const editor = editorWith([paragraph("before"), table, paragraph("after")]);
  const names = () => editor.state.doc.content.content.map((node) => node.textContent);
  const tablePos = editor.state.doc.child(0).nodeSize;
  assert.equal(moveBlock(editor, tablePos, 1), true);
  assert.deepEqual(names().slice(0, 3), ["before", "after", "AB12"]);
  const moved = editor.state.doc.child(0).nodeSize + editor.state.doc.child(1).nodeSize;
  assert.equal(editor.state.doc.nodeAt(moved)?.type.name, "table");
  assert.equal(moveBlock(editor, moved, -1), true);
  assert.equal(moveBlock(editor, tablePos, -1), true);
  assert.deepEqual(names().slice(0, 3), ["AB12", "before", "after"]);
  assert.equal(moveBlock(editor, 0, -1), false, "the first block cannot move up");
});

test("the code-block chrome reads the block at the caret", () => {
  const editor = editorWith([
    paragraph("out"),
    { type: "codeBlock", attrs: { language: "typescript", id: "code-1" }, content: [text("const a = 1;")] },
  ]);
  editor.commands.setTextSelection(posOfText(editor, "out") + 1);
  assert.equal(codeBlockAtCaret(editor), null);
  editor.commands.setTextSelection(posOfText(editor, "const a = 1;") + 2);
  assert.deepEqual(codeBlockAtCaret(editor), {
    id: "code-1",
    editable: true,
    language: "typescript",
    highlightLines: [],
    text: "const a = 1;",
  });
});

test("the code-block chrome's gutter numbers lines and paints highlighted and diff lines", () => {
  const block = { id: "b", editable: true, language: "typescript", highlightLines: [2], text: "one\ntwo\nthree" };
  // Highlighted lines paint even without line numbers.
  assert.deepEqual(codeChromeView(block, DEFAULT_CODE_CHROME), {
    language: "typescript",
    foldable: false,
    lines: [
      { n: 1, label: " ", kind: null },
      { n: 2, label: " ", kind: "meta" },
      { n: 3, label: " ", kind: null },
    ],
  });
  // A fence's {n-m} applies when the attribute has none; line numbers label every line.
  const fenced = { ...block, language: "ts {1-2}", highlightLines: [] };
  assert.deepEqual(
    codeChromeView(fenced, { ...DEFAULT_CODE_CHROME, linenos: true }).lines?.map((line) => [line.label, line.kind]),
    [
      ["1", "meta"],
      ["2", "meta"],
      ["3", null],
    ],
  );
  const diff = { ...block, language: "diff", highlightLines: [], text: "-old\n+new\n same" };
  assert.deepEqual(
    codeChromeView(diff, DEFAULT_CODE_CHROME).lines?.map((line) => line.kind),
    ["del", "add", null],
  );
  // Nothing to show: no gutter at all.
  assert.equal(codeChromeView({ ...block, highlightLines: [] }, DEFAULT_CODE_CHROME).lines, null);
  // A block past the highlight limit is not painted.
  const huge = { ...diff, text: `+${"x".repeat(HIGHLIGHT_MAX_CHARS)}` };
  assert.equal(codeChromeView(huge, DEFAULT_CODE_CHROME).lines, null);
});

test("the code-block chrome offers folding from nine lines on", () => {
  const lines = (count: number) => Array.from({ length: count }, (_, i) => `line ${i + 1}`).join("\n");
  const block = { id: "b", editable: true, language: "", highlightLines: [], text: lines(8) };
  assert.equal(codeChromeView(block, DEFAULT_CODE_CHROME).foldable, false);
  assert.equal(codeChromeView({ ...block, text: lines(9) }, DEFAULT_CODE_CHROME).foldable, true);
  assert.equal(codeChromeView({ ...block, text: "" }, { ...DEFAULT_CODE_CHROME, linenos: true }).lines?.length, 1);
});
