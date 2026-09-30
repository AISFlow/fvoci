import type { Editor } from "@tiptap/core";
import type { Node } from "@tiptap/pm/model";
import { computed, onScopeDispose, type Ref, shallowRef, watch, watchEffect } from "vue";
import { isInsideTable, isTableBlock, plusAt } from "../gutter-actions.js";
import { useEditorState } from "./use-editor-state.js";

/** The block a gutter acts on; pos -1 when there is none. */
export type GutterBlock = { node: Node | null; pos: number };

/** The drag handle the editor starts with (FvociEditor.vue): its element,
 * which the gutter buttons render into, and the block it is at. */
export type GutterHandle = {
  readonly element: HTMLElement;
  readonly block: Readonly<Ref<GutterBlock>>;
};

/** Where the block menu opens, and for which block. */
export type BlockMenuAnchor = { x: number; y: number; pos: number };

/** The caret's block, which the keyboard gutter button acts on; -1 inside or
 * on a table (the table handles own tables). */
export function keyboardBlockPos(editor: Editor): number {
  const $pos = editor.state.selection.$from;
  const pos = $pos.depth ? $pos.before($pos.depth) : $pos.pos;
  return isInsideTable(editor, pos) || isTableBlock(editor.state.doc.nodeAt(pos)) ? -1 : pos;
}

/** Moving the pointer further than this (squared px) makes a press a drag. */
const DRAG_SLOP = 16;
/** A touch held this long shows the gutter on narrow screens. */
const TOUCH_HOLD_MS = 500;

/**
 * The block gutter's behaviour (react/gutter.tsx): the hovered block from
 * the drag handle, the keyboard button's block, the block menu (opened
 * from the handle, the keyboard button or a context menu on an empty
 * selection), touch hold, and the drag handle's classes. Every edit goes
 * through editor commands (gutter-actions.ts), so Yjs carries it to peers.
 * Listeners on the editor go with the calling scope.
 */
export function useBlockGutter(editor: Editor, handle: GutterHandle) {
  const keyboardPos = useEditorState(editor, keyboardBlockPos);
  const block = shallowRef<GutterBlock>(handle.block.value);
  watch(handle.block, (next) => {
    block.value = next;
  });
  const menu = shallowRef<BlockMenuAnchor | null>(null);
  const touch = shallowRef(false);
  const inTable = computed(
    () =>
      isTableBlock(block.value.node) ||
      (block.value.pos >= 0 && isInsideTable(editor, block.value.pos)),
  );

  watchEffect(() => {
    handle.element.classList.toggle("fvoci-gutter-hidden", inTable.value);
    handle.element.classList.toggle("fvoci-gutter-touch", touch.value);
  });

  function openAt(pos: number, x: number, y: number): void {
    if (!editor.isEditable || editor.view.composing) return;
    block.value = { node: editor.state.doc.nodeAt(pos), pos };
    menu.value = { x, y, pos };
  }

  // Official useEditorDragHandle pattern: keep the hovered block fixed
  // while its menu owns focus. The creation-time plugin remains the owner.
  watch(
    menu,
    (value) => {
      if (!editor.isDestroyed)
        editor
          .chain()
          .setMeta("lockDragHandle", value !== null)
          .run();
    },
    { flush: "sync" },
  );
  onScopeDispose(() => {
    if (!editor.isDestroyed) editor.chain().setMeta("lockDragHandle", false).run();
  });

  const dom = editor.view.dom;
  const onContextMenu = (event: MouseEvent) => {
    if (!editor.isEditable || editor.view.composing) return;
    if (!editor.state.selection.empty) return;
    const hit = editor.view.posAtCoords({ left: event.clientX, top: event.clientY });
    if (hit === null) return;
    const $pos = editor.state.doc.resolve(hit.pos);
    if ($pos.depth === 0) return;
    const pos = $pos.before($pos.depth);
    if (isInsideTable(editor, pos)) return;
    event.preventDefault();
    openAt(pos, event.clientX, event.clientY);
  };
  let hold = 0;
  const onPointerDown = (event: PointerEvent) => {
    if (event.pointerType !== "touch") return;
    hold = window.setTimeout(() => {
      touch.value = true;
    }, TOUCH_HOLD_MS);
  };
  const onPointerUp = () => {
    window.clearTimeout(hold);
    touch.value = false;
  };
  dom.addEventListener("contextmenu", onContextMenu);
  dom.addEventListener("pointerdown", onPointerDown);
  dom.ownerDocument.addEventListener("pointerup", onPointerUp);
  onScopeDispose(() => {
    dom.removeEventListener("contextmenu", onContextMenu);
    dom.removeEventListener("pointerdown", onPointerDown);
    dom.ownerDocument.removeEventListener("pointerup", onPointerUp);
    window.clearTimeout(hold);
  });

  // A press that moved, or became a native drag, is not a click on the handle.
  let suppressed = false;
  let press: { x: number; y: number } | null = null;

  return {
    keyboardPos,
    block,
    menu,
    inTable,
    /** The keyboard button (not the drag handle) opens the menu for the caret's block. */
    openFromKeyboard(button: HTMLElement): void {
      const pos = keyboardPos.value;
      if (pos < 0) return;
      const rect = button.getBoundingClientRect();
      openAt(pos, rect.left, rect.bottom);
    },
    closeMenu(): void {
      menu.value = null;
    },
    /** "+": a new block below (or the slash menu in an empty one). The
     * handle is not draggable while "+" is pressed. */
    plus: {
      pointerdown(event: PointerEvent): void {
        event.stopPropagation();
        handle.element.draggable = false;
      },
      pointerup(): void {
        handle.element.draggable = true;
      },
      click(event: MouseEvent): void {
        event.preventDefault();
        if (block.value.pos < 0 || !editor.isEditable || editor.view.composing) return;
        plusAt(editor, block.value.pos);
      },
    },
    /** "⠿": drags the block (the drag handle plugin), or opens the block menu when clicked. */
    drag: {
      pointerdown(event: PointerEvent): void {
        suppressed = false;
        press = { x: event.clientX, y: event.clientY };
      },
      pointermove(event: PointerEvent): void {
        const start = press;
        if (start && (event.clientX - start.x) ** 2 + (event.clientY - start.y) ** 2 > DRAG_SLOP)
          suppressed = true;
      },
      dragstart(): void {
        suppressed = true;
      },
      pointercancel(): void {
        suppressed = true;
        press = null;
      },
      click(event: MouseEvent): void {
        press = null;
        // detail 0: a keyboard click, never suppressed.
        if (suppressed && event.detail !== 0) {
          suppressed = false;
          return;
        }
        if (block.value.pos < 0 || !editor.isEditable || editor.view.composing) return;
        const button = event.currentTarget;
        if (!(button instanceof HTMLElement)) return;
        button.focus({ preventScroll: true });
        const rect = button.getBoundingClientRect();
        menu.value = { x: rect.left, y: rect.bottom, pos: block.value.pos };
      },
    },
  };
}
