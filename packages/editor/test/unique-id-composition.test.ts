import assert from "node:assert/strict";
import test from "node:test";
import { Editor } from "@tiptap/core";
import { EditorState, TextSelection } from "@tiptap/pm/state";
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
 * synced: UniqueID then waits for a "synced" event that never comes and
 * assigns no ids up front. */
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

test("a composition update leaves a block without an id alone; the next plain edit gives it one", () => {
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

test("a composition in an empty block without an id leaves it alone too", () => {
  let state = withPlugins(["첫 문단", ""]);
  // The empty paragraph opens at 6, so its content starts at 7.
  state = state.apply(state.tr.insertText("ㅎ", 7).setMeta("composition", 1));
  assert.equal(state.doc.child(1).textContent, "ㅎ");
  assert.deepEqual(idsOf(state), [null, null]);
});

test("a plain local edit still gives the block an id in the same dispatch", () => {
  let state = withPlugins(["첫 문단", ""]);
  state = state.apply(state.tr.insertText("x", 5));
  const [first, second] = idsOf(state);
  assert.equal(typeof first, "string");
  assert.equal(second, null);
});

test("a Yjs-origin change still gives no id", () => {
  let state = withPlugins(["첫 문단"]);
  state = state.apply(
    state.tr.insertText("원격", 5).setMeta(ySyncPluginKey, { isChangeOrigin: true }),
  );
  assert.deepEqual(idsOf(state), [null]);
});

test("splitting a block outside a composition still leaves two distinct ids", () => {
  let state = withPlugins(["첫 문단"], ["block-a"]);
  const tr = state.tr.setSelection(TextSelection.create(state.doc, 2)).split(2);
  state = state.apply(tr);
  const ids = idsOf(state);
  assert.equal(ids.length, 2);
  assert.ok(ids.includes("block-a"));
  assert.equal(new Set(ids).size, 2);
  assert.ok(ids.every((id) => typeof id === "string"));
});
