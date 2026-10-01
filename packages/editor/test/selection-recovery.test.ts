import assert from "node:assert/strict";
import test from "node:test";
import { Schema } from "@tiptap/pm/model";
import { EditorState, TextSelection, type Plugin } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import {
  getRelativeSelection,
  initProseMirrorDoc,
  prosemirrorToYDoc,
  relativePositionToAbsolutePosition,
  ySyncPlugin,
  ySyncPluginKey,
  type ProsemirrorBinding,
} from "@tiptap/y-tiptap";
import * as Y from "yjs";

// Exercise the installed binding, not a copy of its recovery predicates.
const schema = new Schema({
  nodes: {
    doc: { content: "paragraph+" },
    paragraph: { content: "text*", attrs: { id: { default: null } } },
    text: {},
  },
});

function binding(texts: string[], caret: number, ids = texts.map((_, i) => `block-${String(i)}`)) {
  const source = prosemirrorToYDoc(
    schema.node(
      "doc",
      null,
      texts.map((text, i) =>
        schema.node("paragraph", { id: ids[i] }, text ? schema.text(text) : undefined),
      ),
    ),
  );
  const replica = new Y.Doc({ gc: false });
  Y.applyUpdate(replica, Y.encodeStateAsUpdate(source));
  const fragment = replica.getXmlFragment("prosemirror");
  const init = initProseMirrorDoc(fragment, schema);
  const sync = ySyncPlugin(fragment, { mapping: init.mapping }) as unknown as Plugin;
  let state = EditorState.create({
    schema,
    doc: init.doc,
    plugins: [sync],
    selection: TextSelection.create(init.doc, caret),
  });
  const host = {
    state,
    hasFocus: () => false,
    dispatch(tr: import("@tiptap/pm/state").Transaction) {
      state = state.apply(tr);
      host.state = state;
    },
  };
  assert.ok(sync.spec.view);
  const view = sync.spec.view(host as unknown as EditorView);
  return {
    source,
    get state() {
      return state;
    },
    local(tr: import("@tiptap/pm/state").Transaction) {
      const before = state;
      host.dispatch(tr);
      view.update?.(host as unknown as EditorView, before);
      Y.applyUpdate(source, Y.encodeStateAsUpdate(replica));
    },
    remote(edit: (root: Y.XmlFragment) => void) {
      source.transact(() => {
        edit(source.getXmlFragment("prosemirror"));
      });
      Y.applyUpdate(replica, Y.encodeStateAsUpdate(source));
    },
    destroy() {
      view.destroy?.();
      replica.destroy();
      source.destroy();
    },
  };
}

await test("remote prefix follows the original-text caret between local Backspaces", () => {
  const live = binding(["한글본문"], 5);
  try {
    live.local(live.state.tr.delete(4, 5));
    assert.equal(live.state.selection.from, 4);
    live.remote((root) => {
      ((root.get(0) as Y.XmlElement).get(0) as Y.XmlText).insert(0, "앞쪽삽");
    });
    assert.equal(live.state.doc.textContent, "앞쪽삽한글본");
    assert.equal(live.state.selection.from, 7);
    const pos = live.state.selection.from;
    assert.equal(live.state.doc.textBetween(pos - 1, pos), "본");
    live.local(live.state.tr.delete(pos - 1, pos));
    live.remote((root) => {
      ((root.get(0) as Y.XmlElement).get(0) as Y.XmlText).insert(3, "입");
    });
    assert.equal(live.state.doc.textContent, "앞쪽삽입한글");
  } finally {
    live.destroy();
  }
});

await test("remote inline deletion follows content instead of retaining the old offset", () => {
  const live = binding(["앞쪽한글본문"], 5);
  try {
    live.remote((root) => {
      ((root.get(0) as Y.XmlElement).get(0) as Y.XmlText).delete(0, 2);
    });
    assert.equal(live.state.selection.from, 3);
    assert.equal(live.state.doc.textContent, "한글본문");
  } finally {
    live.destroy();
  }
});

await test("both endpoints of a backward text selection follow remote inline insertion", () => {
  const live = binding(["한글본문"], 5);
  try {
    live.local(live.state.tr.setSelection(TextSelection.create(live.state.doc, 5, 3)));
    live.remote((root) => {
      ((root.get(0) as Y.XmlElement).get(0) as Y.XmlText).insert(0, "앞쪽");
    });
    assert.equal(live.state.selection.anchor, 7);
    assert.equal(live.state.selection.head, 5);
    assert.equal(
      live.state.doc.textBetween(live.state.selection.from, live.state.selection.to),
      "본문",
    );
  } finally {
    live.destroy();
  }
});

await test("an inline prefix follows the caret when the same update also inserts a sibling block", () => {
  const live = binding(["한글본"], 4);
  try {
    live.remote((root) => {
      ((root.get(0) as Y.XmlElement).get(0) as Y.XmlText).insert(0, "앞쪽삽");
      const sibling = new Y.XmlElement("paragraph");
      sibling.setAttribute("id", "unrelated");
      const text = new Y.XmlText();
      text.insert(0, "다른");
      sibling.insert(0, [text]);
      root.insert(1, [sibling]);
    });
    assert.equal(live.state.selection.from, 7);
    assert.equal(live.state.selection.$from.parent.attrs.id, "block-0");
    assert.equal(live.state.doc.textBetween(6, 7), "본");
  } finally {
    live.destroy();
  }
});

await test("delete-and-insert block move still recovers its text offset and distinct ID", () => {
  const live = binding(["다른", "한글본문"], 7);
  try {
    const { binding: currentBinding } = ySyncPluginKey.getState(live.state) as {
      binding: ProsemirrorBinding;
    };
    const saved = getRelativeSelection(currentBinding, live.state);
    live.remote((root) => {
      const block = (root.get(1) as Y.XmlElement).clone();
      root.delete(1, 1);
      root.insert(0, [block]);
    });
    assert.equal(
      relativePositionToAbsolutePosition(
        currentBinding.doc,
        currentBinding.type,
        saved.anchor,
        currentBinding.mapping,
      ),
      10,
      "the deleted type resolves into a sibling; structural recovery must supply the endpoint",
    );
    assert.equal(live.state.selection.$from.parent.attrs.id, "block-1");
    assert.equal(live.state.selection.$from.parentOffset, 2);
    assert.equal(live.state.selection.from, 3);
  } finally {
    live.destroy();
  }
});

await test("an unavailable saved relative endpoint still uses structural block recovery", () => {
  const live = binding(["한글본문"], 3);
  // Model a stored relative position whose Yjs item is unavailable locally.
  // This is a controlled binding-state test, not a native browser selection.
  const unavailable = prosemirrorToYDoc(
    schema.node("doc", null, [
      schema.node("paragraph", { id: "foreign" }, schema.text("한글본문")),
    ]),
  );
  try {
    const { binding: currentBinding } = ySyncPluginKey.getState(live.state) as {
      binding: ProsemirrorBinding;
    };
    const saved = getRelativeSelection(currentBinding, live.state);
    const text = (unavailable.getXmlFragment("prosemirror").get(0) as Y.XmlElement).get(
      0,
    ) as Y.XmlText;
    const relative = Y.createRelativePositionFromTypeIndex(text, 2);
    assert.equal(
      relativePositionToAbsolutePosition(
        currentBinding.doc,
        currentBinding.type,
        relative,
        currentBinding.mapping,
      ),
      null,
    );
    currentBinding.beforeTransactionSelection = { ...saved, anchor: relative, head: relative };
    live.remote((root) => {
      const block = (root.get(0) as Y.XmlElement).clone();
      root.delete(0, 1);
      root.insert(0, [block]);
    });
    assert.equal(live.state.selection.from, 3);
    assert.equal(live.state.selection.$from.parent.attrs.id, "block-0");
  } finally {
    unavailable.destroy();
    live.destroy();
  }
});

await test("same-text siblings retain the moved block's distinct ID", () => {
  const live = binding(["한글본문", "한글본문"], 9);
  try {
    live.remote((root) => {
      const block = (root.get(1) as Y.XmlElement).clone();
      root.delete(1, 1);
      root.insert(0, [block]);
    });
    assert.equal(live.state.selection.$from.parent.attrs.id, "block-1");
    assert.equal(live.state.selection.$from.parentOffset, 2);
  } finally {
    live.destroy();
  }
});
