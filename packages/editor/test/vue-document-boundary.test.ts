import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { runInNewContext } from "node:vm";
import { EditorState, TextSelection, type Transaction } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import { getSchema } from "@tiptap/core";
import ts from "typescript";
import { createFvociExtensions } from "../src/tiptap-schema.ts";

// Execute the SFC's real handlers against PM state, as the mode tests do.
const source = readFileSync(new URL("../src/vue/FvociEditor.vue", import.meta.url), "utf8");
const script = source.split('<script setup lang="ts">')[1]?.split("</script>")[0];
assert.ok(script);
const parsed = ts.createSourceFile("FvociEditor.ts", script, ts.ScriptTarget.Latest, true);
const handlers = parsed.statements.filter(
  (statement) =>
    ts.isFunctionDeclaration(statement) &&
    ["settleNativeLineBoundary", "nativeKeyboardSelection"].includes(statement.name?.text ?? ""),
);
assert.equal(handlers.length, 2);
const schema = getSchema(createFvociExtensions());

function fixture(
  options: {
    apple?: boolean;
    ios?: boolean;
    composing?: boolean;
    nativeAnchor?: number;
    edges?: boolean;
  } = {},
) {
  const paragraphs = ["첫 문단", "중간 문단", "끝 문단"].map((text) => ({
    type: "paragraph",
    content: [{ type: "text", text }],
  }));
  const doc = schema.nodeFromJSON({
    type: "doc",
    content: options.edges
      ? [
          { type: "horizontalRule" },
          {
            type: "bulletList",
            content: paragraphs.map((paragraph) => ({ type: "listItem", content: [paragraph] })),
          },
          { type: "horizontalRule" },
        ]
      : paragraphs,
  });
  const initial = options.edges ? 12 : 10;
  let state = EditorState.create({ doc, selection: TextSelection.create(doc, initial) });
  const native = {
    anchorNode: {},
    focusNode: {},
    anchorOffset: options.nativeAnchor ?? initial,
    focusOffset: initial,
    // documentboundary must not depend on this optional browser API.
  };
  let dispatches = 0;
  let scrolled = false;
  const view = {
    get state() {
      return state;
    },
    editable: true,
    composing: false,
    hasFocus: () => true,
    dom: { contains: () => true, ownerDocument: { getSelection: () => native } },
    posAtDOM: (_node: unknown, offset: number) => offset,
    focus() {
      native.anchorOffset = state.selection.anchor;
      native.focusOffset = state.selection.head;
    },
    dispatch(tr: Transaction) {
      dispatches++;
      scrolled = tr.scrolledIntoView;
      state = state.apply(tr);
    },
  };
  const handle = runInNewContext(
    ts.transpileModule(
      `${handlers.map((handler) => handler.getText(parsed)).join("\n")}\nsettleNativeLineBoundary;`,
      { compilerOptions: { target: ts.ScriptTarget.ES2023, module: ts.ModuleKind.ESNext } },
    ).outputText,
    {
      TextSelection,
      Element: class {
        parentElement = null;
      },
      richComposing: { value: options.composing ?? false },
      isMacOS: () => options.apple ?? false,
      isiOS: () => options.ios ?? false,
    },
  ) as (view: EditorView, event: KeyboardEvent) => boolean;
  return {
    view,
    native,
    run: (event: Partial<KeyboardEvent> = {}) =>
      handle(
        view as unknown as EditorView,
        {
          key: "Home",
          isTrusted: true,
          ctrlKey: true,
          ...event,
        } as KeyboardEvent,
      ),
    result: () => ({ selection: state.selection, dispatches, scrolled }),
  };
}

for (const key of ["Home", "End"]) {
  for (const shiftKey of [false, true]) {
    await test(`Ctrl${shiftKey ? "+Shift" : ""}+${key} commits the document boundary before keyup without Selection.modify`, () => {
      const f = fixture();
      const before = f.view.state.doc;
      assert.equal(f.run({ key, shiftKey }), true);
      const { selection, dispatches, scrolled } = f.result();
      assert.equal(selection.head, key === "Home" ? 1 : before.content.size - 1);
      assert.equal(selection.anchor, shiftKey ? 10 : selection.head);
      assert.equal(dispatches, 1);
      assert.equal(scrolled, true);
      assert.equal(f.native.anchorOffset, selection.anchor);
      assert.equal(f.native.focusOffset, selection.head);
      assert.equal(f.view.state.doc, before, "navigation never changes document content");
    });
  }
}

await test("Shift document navigation preserves the current native anchor when PM has not caught up", () => {
  const f = fixture({ nativeAnchor: 8 });
  assert.equal(f.run({ shiftKey: true }), true);
  assert.equal(f.result().selection.anchor, 8);
  assert.equal(f.result().selection.head, 1);
});

await test("an unchanged model boundary still restores a native caret that moved before selectionchange", () => {
  const f = fixture();
  f.view.dispatch(f.view.state.tr.setSelection(TextSelection.create(f.view.state.doc, 1)));
  assert.equal(f.native.focusOffset, 10);
  assert.equal(f.run(), true);
  assert.equal(f.result().selection.head, 1);
  assert.equal(f.native.focusOffset, 1);
});

await test("document boundaries find text inside nested lists beyond leading and trailing atoms", () => {
  for (const key of ["Home", "End"]) {
    const f = fixture({ edges: true });
    assert.equal(f.run({ key }), true);
    assert.equal(f.result().selection.head, key === "Home" ? 4 : f.view.state.doc.content.size - 4);
    assert.equal(f.result().selection.$head.parent.isTextblock, true);
  }
});

await test("document boundary navigation leaves Apple, IME, untrusted and modified input untouched", () => {
  const cases = [
    { options: { apple: true }, event: {} },
    { options: { ios: true }, event: {} },
    { options: { composing: true }, event: {} },
    { options: {}, event: { isComposing: true } },
    { options: {}, event: { isTrusted: false } },
    { options: {}, event: { defaultPrevented: true } },
    { options: {}, event: { altKey: true } },
    { options: {}, event: { metaKey: true } },
  ];
  for (const { options, event } of cases) {
    const f = fixture(options);
    assert.equal(f.run(event), false);
    assert.equal(f.result().dispatches, 0);
  }
  for (const guard of ["editable", "composing", "hasFocus"] as const) {
    const f = fixture();
    if (guard === "hasFocus") f.view.hasFocus = () => false;
    else f.view[guard] = guard === "composing";
    assert.equal(f.run(), false);
    assert.equal(f.result().dispatches, 0);
  }
});
