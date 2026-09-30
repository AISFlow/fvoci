import type { Editor } from "@tiptap/core";
import { onScopeDispose, shallowRef, watch } from "vue";
import { moveBlock, moveNodeTo } from "../gutter-actions.js";
import { isInTable, tablePosOf } from "../table-actions.js";
import { useEditorState } from "./use-editor-state.js";

/** The table the caret is in, and the caret. */
export type TableCaret = { tablePos: number; from: number };

/** The handles' box: the table's rectangle in the editor host's coordinates. */
export type TableHandlesBox = { left: number; top: number; width: number; height: number };

export function tableCaret(editor: Editor): TableCaret | null {
	return isInTable(editor) ? { tablePos: tablePosOf(editor), from: editor.state.selection.from } : null;
}

function tableDom(editor: Editor, pos: number): HTMLElement | null {
	const dom = editor.view.nodeDOM(pos);
	if (dom instanceof HTMLElement) return dom;
	if (dom instanceof Node && dom.parentElement) return dom.parentElement;
	return null;
}

/** `table` in the coordinates of `host` (an offset parent), scroll and border included. */
export function boxInHost(
	table: { left: number; top: number; width: number; height: number },
	origin: { left: number; top: number },
	host: { clientLeft: number; clientTop: number; scrollLeft: number; scrollTop: number },
): TableHandlesBox {
	return {
		left: table.left - origin.left - host.clientLeft + host.scrollLeft,
		top: table.top - origin.top - host.clientTop + host.scrollTop,
		width: table.width,
		height: table.height,
	};
}

/** Moving the pointer further than this (squared px) makes a press on the table handle a drag. */
const DRAG_SLOP = 16;

/**
 * The table handles' behaviour (react/table-handles.tsx): shown while the
 * caret is in a table, placed over it inside the editor host (re-measured
 * on scroll and resize), dragging the table handle moves the table to the
 * block under the pointer, and the handles open the table menu. Edits go
 * through editor commands (table-actions.ts), so Yjs carries them to peers.
 */
export function useTableHandles(editor: Editor) {
	const caret = useEditorState(editor, tableCaret);
	const box = shallowRef<TableHandlesBox | null>(null);
	const menu = shallowRef<{ x: number; y: number } | null>(null);
	const tick = shallowRef(0);

	function measure(): void {
		const inside = caret.value;
		if (!inside || inside.tablePos < 0) {
			box.value = null;
			return;
		}
		const table = tableDom(editor, inside.tablePos);
		if (!table) {
			box.value = null;
			return;
		}
		// WHY: Keep coordinates local to the editor even inside a translated dialog.
		const host = editor.view.dom.closest<HTMLElement>(".fvoci-editor");
		if (!host) return;
		const next = boxInHost(table.getBoundingClientRect(), host.getBoundingClientRect(), host);
		const prev = box.value;
		if (
			prev &&
			prev.left === next.left &&
			prev.top === next.top &&
			prev.width === next.width &&
			prev.height === next.height
		)
			return;
		box.value = next;
	}
	watch([caret, tick], measure, { immediate: true, flush: "post" });

	const onScroll = () => {
		tick.value += 1;
	};
	document.addEventListener("scroll", onScroll, true);
	window.addEventListener("resize", onScroll);
	onScopeDispose(() => {
		document.removeEventListener("scroll", onScroll, true);
		window.removeEventListener("resize", onScroll);
	});

	let drag: { x: number; y: number; tablePos: number; moved: boolean } | null = null;
	// A press on the table handle that moved (or was cancelled) is not a click.
	let suppressed = false;

	return {
		caret,
		box,
		menu,
		closeMenu(): void {
			menu.value = null;
		},
		/** Any handle opens the menu below itself, except a table handle press that dragged. */
		openMenu(event: MouseEvent): void {
			if (!editor.isEditable || editor.view.composing) return;
			const button = event.currentTarget;
			if (!(button instanceof HTMLElement)) return;
			if (button.dataset.tableHandle === "table" && suppressed && event.detail !== 0) {
				suppressed = false;
				return;
			}
			button.focus({ preventScroll: true });
			const rect = button.getBoundingClientRect();
			menu.value = { x: rect.left, y: rect.bottom };
		},
		/** Dragging the table handle. */
		drag: {
			pointerdown(event: PointerEvent): void {
				if (!editor.isEditable || editor.view.composing) return;
				const inside = caret.value;
				const button = event.currentTarget;
				if (!inside || !(button instanceof HTMLElement)) return;
				event.preventDefault();
				button.focus({ preventScroll: true });
				suppressed = false;
				event.stopPropagation();
				drag = { x: event.clientX, y: event.clientY, tablePos: inside.tablePos, moved: false };
				button.setPointerCapture(event.pointerId);
			},
			pointermove(event: PointerEvent): void {
				const start = drag;
				if (!start) return;
				const dx = event.clientX - start.x;
				const dy = event.clientY - start.y;
				if (dx * dx + dy * dy > DRAG_SLOP) start.moved = true;
			},
			pointerup(event: PointerEvent): void {
				const start = drag;
				drag = null;
				if (!start) return;
				suppressed = start.moved;
				if (!start.moved || !editor.isEditable || editor.view.composing) return;
				const hit = editor.view.posAtCoords({ left: event.clientX, top: event.clientY });
				if (!hit) return;
				const $pos = editor.state.doc.resolve(hit.pos);
				const insertPos = $pos.depth === 0 ? hit.pos : $pos.before(1);
				moveNodeTo(editor, start.tablePos, insertPos);
			},
			pointercancel(): void {
				drag = null;
				suppressed = true;
			},
		},
		/** Moves the table past the block before or after it (the table menu).
		 * WHY: react/table-handles.tsx moves to `tablePos - 1` / `tablePos +
		 * nodeSize`, which moveNodeTo maps back to where the table is: there,
		 * both items leave the document unchanged. moveBlock swaps the table
		 * with its sibling, as the block menu's up/down do. */
		moveTable(dir: -1 | 1): void {
			if (!editor.isEditable || editor.view.composing) return;
			const inside = caret.value;
			if (!inside) return;
			moveBlock(editor, inside.tablePos, dir);
		},
	};
}
