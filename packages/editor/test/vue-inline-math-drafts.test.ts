import assert from "node:assert/strict";
import test from "node:test";
import { getSchema, type Editor } from "@tiptap/core";
import { EditorState, Plugin } from "@tiptap/pm/state";
import { initProseMirrorDoc, ySyncPluginKey } from "@tiptap/y-tiptap";
import * as Y from "yjs";
import { FVOCI_YDOC_FRAGMENT } from "../src/collab/constants.ts";
import { tiptapJsonToYDoc } from "../src/collab-tiptap.ts";
import { createFvociExtensions } from "../src/tiptap-schema.ts";
import { inlineMathDrafts } from "../src/vue/inline-math-drafts.ts";

const schema = getSchema(createFvociExtensions());
const draft = { value: "local", focused: true, start: 1, end: 3, direction: "backward" as const };
const math = (latex: string) => ({ type: "mathInline", attrs: { latex } });

// The real Yjs fragment and exported y-tiptap mapping, with only the host's
// event subscriptions stubbed. Browser tests exercise the actual Vue views.
function harness() {
  const ydoc = tiptapJsonToYDoc({
    type: "doc",
    content: [{ type: "paragraph", content: [math("x"), math("y")] }],
  });
  const type = ydoc.getXmlFragment(FVOCI_YDOC_FRAGMENT);
  const listeners = new Map<string, Set<() => void>>();
  const host = {
    state: {} as EditorState,
    isEditable: true,
    on(event: string, fn: () => void) {
      const set = listeners.get(event) ?? new Set();
      set.add(fn);
      listeners.set(event, set);
    },
    off(event: string, fn: () => void) {
      listeners.get(event)?.delete(fn);
    },
  };
  const emit = (event: string) => {
    for (const fn of [...(listeners.get(event) ?? [])]) fn();
  };
  const refresh = () => {
    const { doc, mapping } = initProseMirrorDoc(type, schema);
    const sync = { doc: ydoc, type, binding: { mapping } };
    host.state = EditorState.create({
      schema,
      doc,
      plugins: [
        new Plugin({ key: ySyncPluginKey, state: { init: () => sync, apply: () => sync } }),
      ],
    });
    emit("transaction");
  };
  refresh();
  return { editor: host as unknown as Editor, host, ydoc, type, refresh, emit, listeners };
}

await test("adjacent atoms and editors have independent drafts, including an atom at the paragraph start", () => {
  const a = harness();
  const b = harness();
  const first = inlineMathDrafts(a.editor, () => 1);
  const second = inlineMathDrafts(a.editor, () => 2);
  first.write(draft);
  assert.deepEqual(inlineMathDrafts(a.editor, () => 1).read(), draft);
  assert.equal(second.read(), undefined);
  assert.equal(inlineMathDrafts(b.editor, () => 1).read(), undefined);
  a.emit("destroy");
  b.emit("destroy");
  a.ydoc.destroy();
  b.ydoc.destroy();
});

await test("the same Yjs atom retains its draft after attributes and preceding text change", () => {
  const a = harness();
  const field = inlineMathDrafts(a.editor, () => 1);
  field.write(draft);
  const paragraph = a.type.get(0) as Y.XmlElement;
  const atom = paragraph.get(0) as Y.XmlElement;
  atom.setAttribute("latex", "remote");
  const prefix = new Y.XmlText();
  prefix.insert(0, "앞");
  paragraph.insert(0, [prefix]);
  a.refresh();
  assert.deepEqual(inlineMathDrafts(a.editor, () => 2).read(), draft);
  prefix.delete(0, 1);
  a.refresh();
  assert.deepEqual(inlineMathDrafts(a.editor, () => 1).read(), draft);
  a.emit("destroy");
  a.ydoc.destroy();
});

await test("a deleted atom cannot write or commit into a reused view or a replacement at the same position", () => {
  const a = harness();
  const reused = inlineMathDrafts(a.editor, () => 1);
  reused.write(draft);
  const paragraph = a.type.get(0) as Y.XmlElement;
  paragraph.delete(0, 1);
  a.refresh();
  assert.equal(reused.isCurrent(), false);
  reused.write(draft);
  assert.equal(inlineMathDrafts(a.editor, () => 1).read(), undefined);
  reused.begin();
  assert.equal(reused.isCurrent(), true);
  reused.write({ ...draft, value: "second" });
  assert.equal(inlineMathDrafts(a.editor, () => 1).read()?.value, "second");
  const replacement = new Y.XmlElement("mathInline");
  replacement.setAttribute("latex", "new");
  paragraph.insert(0, [replacement]);
  a.refresh();
  assert.equal(inlineMathDrafts(a.editor, () => 1).read(), undefined);
  a.emit("destroy");
  a.ydoc.destroy();
});

await test("permission loss and editor destruction clear drafts and owned listeners", () => {
  const a = harness();
  const field = inlineMathDrafts(a.editor, () => 1);
  field.write(draft);
  a.host.isEditable = false;
  a.emit("update");
  assert.equal(field.read(), undefined);
  a.host.isEditable = true;
  field.write(draft);
  a.emit("destroy");
  assert.equal(field.read(), undefined);
  assert.equal(a.listeners.get("transaction")?.size, 0);
  assert.equal(a.listeners.get("update")?.size, 0);
  a.ydoc.destroy();
});
