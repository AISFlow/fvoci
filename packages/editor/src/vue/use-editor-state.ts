import type { Editor } from "@tiptap/core";
import { onScopeDispose, type Ref, shallowRef } from "vue";

/** Structural equality of plain values (primitives, arrays, plain objects);
 * anything else — a ProseMirror node, say — compares by identity. */
export function sameValue(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== "object" || typeof b !== "object" || a === null || b === null) return false;
  const proto: unknown = Object.getPrototypeOf(a);
  if (proto !== Object.getPrototypeOf(b)) return false;
  if (proto !== Object.prototype && proto !== Array.prototype) return false;
  const left = a as Record<string, unknown>;
  const right = b as Record<string, unknown>;
  const keys = Object.keys(left);
  if (keys.length !== Object.keys(right).length) return false;
  return keys.every((key) => Object.hasOwn(right, key) && sameValue(left[key], right[key]));
}

/**
 * A value derived from the editor, recomputed after each transaction and
 * each editable change (the React controls' useEditorState). Reading
 * `editor.state` in a computed instead would lag: @tiptap/vue-3 triggers
 * its reactive state two animation frames after the transaction. The ref
 * changes only when the value does (sameValue), so an unrelated keystroke
 * re-renders nothing. The listeners go with the calling scope.
 */
export function useEditorState<T>(editor: Editor, select: (editor: Editor) => T): Readonly<Ref<T>> {
  const value = shallowRef(select(editor));
  // A destroyed editor emits nothing more (destroy removes its listeners).
  const sync = () => {
    const next = select(editor);
    if (!sameValue(value.value, next)) value.value = next;
  };
  editor.on("transaction", sync);
  // setEditable emits "update" without a transaction event.
  editor.on("update", sync);
  onScopeDispose(() => {
    editor.off("transaction", sync);
    editor.off("update", sync);
  });
  return value;
}
