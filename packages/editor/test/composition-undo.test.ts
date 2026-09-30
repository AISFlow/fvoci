import assert from "node:assert/strict";
import test from "node:test";
import { Editor, getSchema } from "@tiptap/core";
import { EditorState, Plugin } from "@tiptap/pm/state";
import type { EditorView } from "@tiptap/pm/view";
import { ySyncPluginKey, yUndoPluginKey } from "@tiptap/y-tiptap";
import * as Y from "yjs";
import {
  compositionUndoCapture,
  compositionUndoPluginKey,
  createCompositionUndoPlugin,
} from "../src/composition-undo.ts";
import { createFvociEditorExtensions } from "../src/editor-extensions.ts";
import { createFvociExtensions } from "../src/tiptap-schema.ts";

await test("a paused composition undoes and redoes as one operation while peer-origin changes survive", () => {
  const doc = new Y.Doc();
  const text = doc.getText("local");
  const remote = doc.getText("remote");
  remote.insert(0, "peer");
  const manager = new Y.UndoManager([text, remote], { trackedOrigins: new Set(["local"]) });
  const policy = compositionUndoCapture(manager);
  for (const syllable of ["ㅎ", "하", "한"]) {
    // Force elapsed time in the actual UndoManager rather than sleep in a unit test.
    if (manager.lastChange) manager.lastChange -= 650;
    policy.capture(1);
    doc.transact(() => {
      text.delete(0, text.length);
      text.insert(0, syllable);
    }, "local");
    policy.release();
    assert.equal(manager.captureTimeout, 500);
    assert.equal(manager.undoStack.length, 1);
    doc.transact(() => {
      remote.insert(remote.length, "!");
    }, "peer");
    // y-sync may stop capturing during its unrecorded remote selection repair.
    manager.stopCapturing();
  }
  manager.undo();
  assert.equal(text.toJSON(), "");
  assert.equal(remote.toJSON(), "peer!!!");
  manager.redo();
  assert.equal(text.toJSON(), "한");
  assert.equal(remote.toJSON(), "peer!!!");
  manager.destroy();
  doc.destroy();
});

await test("distinct compositions and normal time-based typing remain separate and custom timeout is restored", () => {
  const doc = new Y.Doc();
  const text = doc.getText("local");
  const manager = new Y.UndoManager(text, { captureTimeout: 37 });
  const policy = compositionUndoCapture(manager);
  for (const [id, value] of [
    [1, "한"],
    [2, "글"],
  ] as const) {
    policy.capture(id);
    doc.transact(() => {
      text.insert(text.length, value);
    });
    policy.release();
    policy.release();
    assert.equal(manager.captureTimeout, 37);
  }
  assert.equal(manager.undoStack.length, 2);
  manager.lastChange -= 650;
  text.insert(text.length, "normal");
  assert.equal(manager.undoStack.length, 3);
  manager.undo();
  assert.equal(text.toJSON(), "한글");
  manager.undo();
  assert.equal(text.toJSON(), "한");
  manager.undo();
  assert.equal(text.toJSON(), "");
  policy.capture(3);
  policy.release();
  assert.equal(manager.captureTimeout, 37);
  manager.destroy();
  doc.destroy();
});

await test("no-op writes do not absorb earlier typing, and another top item or undo breaks composition capture", () => {
  const doc = new Y.Doc();
  const text = doc.getText("local");
  const manager = new Y.UndoManager(text);
  text.insert(0, "prior");
  const policy = compositionUndoCapture(manager);
  policy.capture(1);
  doc.transact(() => {});
  policy.release();
  policy.capture(1);
  text.insert(text.length, "한");
  policy.release();
  assert.equal(manager.undoStack.length, 2);
  manager.lastChange -= 650;
  text.insert(text.length, "other");
  policy.capture(1);
  text.insert(text.length, "글");
  policy.release();
  assert.equal(manager.undoStack.length, 4);
  manager.undo();
  assert.equal(text.toJSON(), "prior한other");
  policy.capture(1);
  text.insert(text.length, "새");
  policy.release();
  manager.undo();
  assert.equal(text.toJSON(), "prior한other");
  manager.undo();
  assert.equal(text.toJSON(), "prior한");
  manager.destroy();
  doc.destroy();
});

await test("undoing a newer ordinary item cannot reopen the earlier composition item", () => {
  const doc = new Y.Doc();
  const text = doc.getText("text");
  const manager = new Y.UndoManager(text);
  const policy = compositionUndoCapture(manager);
  try {
    policy.capture(1);
    text.insert(0, "한");
    policy.release();
    const originalItem = manager.undoStack.at(-1);
    manager.stopCapturing();
    text.insert(text.length, "ordinary");
    assert.equal(manager.undoStack.length, 2);
    manager.undo();
    assert.equal(text.toJSON(), "한");
    assert.equal(manager.undoStack.at(-1), originalItem);
    policy.capture(1);
    text.insert(text.length, "글");
    policy.release();
    assert.equal(manager.undoStack.length, 2);
    manager.undo();
    assert.equal(text.toJSON(), "한");
    manager.redo();
    assert.equal(text.toJSON(), "한글");
    manager.undo();
    manager.undo();
    assert.equal(text.toJSON(), "");
  } finally {
    manager.destroy();
    doc.destroy();
  }
});

await test("the installed plugin respects historical undo even when the old item returns to the top", () => {
  const doc = new Y.Doc();
  const text = doc.getText("text");
  const peer = doc.getText("peer");
  const manager = new Y.UndoManager([text, peer], { trackedOrigins: new Set([ySyncPluginKey]) });
  const plugin = createCompositionUndoPlugin();
  let state = EditorState.create({
    schema: getSchema(createFvociExtensions()),
    plugins: [
      new Plugin({
        key: yUndoPluginKey,
        state: { init: () => ({ undoManager: manager }), apply: (_tr, value) => value },
      }),
      plugin,
    ],
  });
  const host = { state };
  assert.ok(plugin.spec.view);
  const view = plugin.spec.view(host as EditorView);
  const apply = (tr: import("@tiptap/pm/state").Transaction) => {
    state = state.apply(tr);
    host.state = state;
  };
  try {
    apply(state.tr.insertText("한", 1).setMeta("composition", 1));
    doc.transact(() => {
      text.insert(0, "한");
    }, ySyncPluginKey);
    const originalItem = manager.undoStack.at(-1);
    manager.stopCapturing();
    apply(state.tr.insertText("ordinary", 2));
    doc.transact(() => {
      text.insert(text.length, "ordinary");
    }, ySyncPluginKey);
    assert.equal(manager.undoStack.length, 2);
    manager.undo();
    apply(
      state.tr
        .delete(2, 10)
        .setMeta(ySyncPluginKey, { isChangeOrigin: true, isUndoRedoOperation: true }),
    );
    assert.equal(manager.undoStack.at(-1), originalItem);
    doc.transact(() => {
      peer.insert(0, "peer");
    }, "peer");
    apply(state.tr.insertText("글", 2).setMeta("composition", 1));
    doc.transact(() => {
      text.insert(text.length, "글");
    }, ySyncPluginKey);
    assert.equal(manager.undoStack.length, 2);
    manager.undo();
    assert.equal(text.toJSON(), "한");
    assert.equal(peer.toJSON(), "peer");
    manager.redo();
    assert.equal(text.toJSON(), "한글");
    assert.equal(peer.toJSON(), "peer");
  } finally {
    assert.ok(view.destroy);
    view.destroy();
    manager.destroy();
    doc.destroy();
  }
});

await test("a composition started after undo keeps grouping when Yjs clears the redo stack", () => {
  const doc = new Y.Doc();
  const text = doc.getText("text");
  const manager = new Y.UndoManager(text, { captureTimeout: 37 });
  const policy = compositionUndoCapture(manager);
  try {
    policy.capture(1);
    text.insert(0, "한");
    policy.release();
    const originalItem = manager.undoStack.at(-1);
    manager.stopCapturing();
    text.insert(text.length, "ordinary");
    manager.undo();
    manager.redo();
    manager.undo();
    assert.equal(manager.undoStack.at(-1), originalItem);
    assert.equal(manager.redoStack.length, 1);
    policy.capture(1);
    text.insert(text.length, "ㄱ");
    policy.release();
    assert.equal(manager.redoStack.length, 0);
    assert.equal(manager.undoStack.length, 2);
    // Remote selection repair still may stop capture after the history boundary.
    manager.stopCapturing();
    policy.capture(1);
    text.delete(1, 1);
    text.insert(1, "글");
    policy.release();
    assert.equal(manager.undoStack.length, 2);
    assert.equal(manager.captureTimeout, 37);
    manager.undo();
    assert.equal(text.toJSON(), "한");
    manager.redo();
    assert.equal(text.toJSON(), "한글");
    manager.clear();
    policy.capture(1);
    text.insert(text.length, "새");
    policy.release();
    manager.undo();
    assert.equal(text.toJSON(), "한글");
  } finally {
    policy.destroy();
    manager.destroy();
    doc.destroy();
  }
});

await test("the plugin wraps actual local Yjs writes and ignores remote, unrecorded and ordinary edits", () => {
  const doc = new Y.Doc();
  const text = doc.getText("text");
  const manager = new Y.UndoManager(text, {
    captureTimeout: 37,
    trackedOrigins: new Set([ySyncPluginKey]),
  });
  const registered: string[] = [];
  const removed: string[] = [];
  const on = manager.on.bind(manager);
  const off = manager.off.bind(manager);
  manager.on = (name, listener) => {
    registered.push(name);
    return on(name, listener);
  };
  manager.off = (name, listener) => {
    removed.push(name);
    off(name, listener);
  };
  const plugin = createCompositionUndoPlugin();
  const schema = getSchema(createFvociExtensions());
  let state = EditorState.create({
    schema,
    plugins: [
      new Plugin({
        key: yUndoPluginKey,
        state: { init: () => ({ undoManager: manager }), apply: (_tr, value) => value },
      }),
      plugin,
    ],
  });
  const host = { state };
  assert.ok(plugin.spec.view);
  const view = plugin.spec.view(host as EditorView);
  const apply = (tr: import("@tiptap/pm/state").Transaction) => {
    state = state.apply(tr);
    host.state = state;
  };
  const observed: number[] = [];
  const observe = () => {
    observed.push(manager.captureTimeout);
  };
  doc.on("beforeTransaction", observe);
  apply(state.tr.insertText("ㅎ", 1).setMeta("composition", 1));
  doc.transact(() => {
    text.insert(0, "ㅎ");
  }, ySyncPluginKey);
  assert.equal(observed.at(-1), Infinity);
  assert.equal(manager.captureTimeout, 37);
  // Reentrant selection-only updates preserve the outer edit's composition ID.
  apply(state.tr.setMeta("awareness", true));
  manager.lastChange -= 650;
  doc.transact(() => {
    text.delete(0, 1);
    text.insert(0, "한");
  }, ySyncPluginKey);
  assert.equal(manager.undoStack.length, 1);
  assert.equal(manager.captureTimeout, 37);
  apply(
    state.tr
      .insertText("remote", 1)
      .setMeta("composition", 1)
      .setMeta(ySyncPluginKey, { isChangeOrigin: true }),
  );
  doc.transact(() => {
    text.insert(0, "peer");
  }, "peer");
  assert.equal(observed.at(-1), 37);
  apply(
    state.tr.insertText("unrecorded", 1).setMeta("composition", 1).setMeta("addToHistory", false),
  );
  doc.transact(() => {
    text.insert(0, "unrecorded");
  }, ySyncPluginKey);
  assert.equal(observed.at(-1), 37);
  apply(state.tr.insertText("ordinary", 1));
  doc.transact(() => {
    text.insert(0, "ordinary");
  }, ySyncPluginKey);
  assert.equal(observed.at(-1), 37);
  assert.ok(view.destroy);
  view.destroy();
  assert.deepEqual(registered, ["stack-item-popped", "stack-cleared"]);
  assert.deepEqual(removed, registered, "destroy removes both owned history listeners");
  apply(state.tr.insertText("한", 1).setMeta("composition", 2));
  doc.transact(() => {
    text.insert(0, "한");
  }, ySyncPluginKey);
  assert.equal(observed.at(-1), 37, "destroy removes the owned listeners");
  doc.off("beforeTransaction", observe);
  manager.destroy();
  doc.destroy();
});

await test("both hosts' real extension list creates collaboration before the composition listener", () => {
  const ydoc = new Y.Doc();
  const stubView = () => () => ({ dom: {} as HTMLElement });
  const nodeViews = { math: stubView, mathInline: stubView, embed: stubView, attachment: stubView };
  const editor = new Editor({
    element: null,
    content: { type: "doc", content: [{ type: "paragraph" }] },
    extensions: createFvociEditorExtensions({
      ydoc,
      nodeViews,
      mentionItems: () => undefined,
      entityResolver: () => null,
      workspaceSlug: () => null,
      uploads: { anchors: new Map(), queue: () => {} },
    }),
  });
  const keys = editor.extensionManager.plugins.map((plugin) => plugin.spec.key);
  const sync = keys.indexOf(ySyncPluginKey);
  const composition = keys.indexOf(compositionUndoPluginKey);
  assert.ok(sync >= 0 && composition > sync);
  editor.destroy();
  ydoc.destroy();
});
