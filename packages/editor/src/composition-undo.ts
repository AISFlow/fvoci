import { isChangeOrigin } from "@tiptap/extension-collaboration";
import { Plugin, PluginKey } from "@tiptap/pm/state";
import { ySyncPluginKey, yUndoPluginKey } from "@tiptap/y-tiptap";
import type { Transaction as YTransaction, UndoManager } from "yjs";

export const compositionUndoPluginKey = new PluginKey<number | null>("fvociCompositionUndo");

/** #260: successive local preedit transactions have the same ProseMirror
 * composition ID. They may join across Yjs's normal capture timeout; all
 * other edits keep the manager's ordinary typing policy. */
export function compositionUndoCapture(manager: UndoManager) {
  let composition: number | undefined;
  let timeout: number | undefined;
  let item: UndoManager["undoStack"][number] | undefined;
  const reset = () => {
    composition = undefined;
    item = undefined;
  };
  const cleared = ({ undoStackCleared }: { undoStackCleared: boolean }) => {
    if (undoStackCleared) reset();
  };
  // Undoing a newer item can expose this composition's old item again.
  // Top identity and the transient undoing/redoing flags cannot detect that
  // history boundary once the next local write starts.
  manager.on("stack-item-popped", reset);
  manager.on("stack-cleared", cleared);
  return {
    capture(id: number): void {
      if (composition !== id || (item && manager.undoStack.at(-1) !== item)) {
        manager.stopCapturing();
        composition = id;
        item = undefined;
      } else if (item && !manager.undoing && !manager.redoing) {
        // y-sync's unrecorded selection repair after a remote update can
        // stop capture. Reopen only this same composition's existing top
        // item; never cross an intervening undo, redo or different item.
        // lastChange is a public field in the pinned Yjs UndoManager type.
        manager.lastChange = Date.now();
      }
      timeout ??= manager.captureTimeout;
      manager.captureTimeout = Number.POSITIVE_INFINITY;
    },
    release(): void {
      if (timeout === undefined) return;
      // No-op Yjs transactions must not claim an earlier typing item.
      if (manager.lastChange > 0 && !manager.undoing && !manager.redoing) {
        item = manager.undoStack.at(-1);
      }
      manager.captureTimeout = timeout;
      timeout = undefined;
    },
    destroy(): void {
      manager.off("stack-item-popped", reset);
      manager.off("stack-cleared", cleared);
    },
  };
}

export function createCompositionUndoPlugin(): Plugin<number | null> {
  const key = compositionUndoPluginKey;
  return new Plugin<number | null>({
    key,
    state: {
      init: () => null,
      apply(transaction, value) {
        if (!transaction.docChanged) return value;
        const id: unknown = transaction.getMeta("composition");
        return !isChangeOrigin(transaction) &&
          transaction.getMeta("addToHistory") !== false &&
          typeof id === "number"
          ? id
          : null;
      },
    },
    view: (view) => {
      const manager: UndoManager = yUndoPluginKey.getState(view.state).undoManager;
      const capture = compositionUndoCapture(manager);
      const before = (transaction: YTransaction) => {
        const id = key.getState(view.state);
        if (transaction.origin === ySyncPluginKey && typeof id === "number") capture.capture(id);
      };
      const after = () => capture.release();
      // Wrap the actual Yjs write, not view.update: awareness can dispatch
      // reentrant selection updates before y-sync writes the outer edit.
      // Collaboration (priority1000) creates the manager before this ordinary
      // priority100 plugin view; its afterTransaction capture runs first.
      manager.doc.on("beforeTransaction", before);
      manager.doc.on("afterTransaction", after);
      return {
        destroy: () => {
          manager.doc.off("beforeTransaction", before);
          manager.doc.off("afterTransaction", after);
          capture.release();
          capture.destroy();
        },
      };
    },
  });
}
