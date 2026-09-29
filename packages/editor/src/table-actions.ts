import type { Editor } from "@tiptap/core";
import { AllSelection, NodeSelection } from "@tiptap/pm/state";
import { CellSelection } from "@tiptap/pm/tables";

const lastRange = new WeakMap<Editor, { table: number; cell: number }>();

function remember(
	editor: Editor,
	patch: Partial<{ table: number; cell: number }>,
): void {
	const prev = lastRange.get(editor) ?? { table: -1, cell: -1 };
	lastRange.set(editor, { ...prev, ...patch });
}

export function tablePosOf(editor: Editor): number {
	const sel = editor.state.selection;
	if (sel instanceof NodeSelection && sel.node.type.name === "table") {
		return sel.from;
	}
	const $pos = editor.state.doc.resolve(sel.from);
	for (let d = $pos.depth; d > 0; d--) {
		if ($pos.node(d).type.name === "table") return $pos.before(d);
	}
	let found = -1;
	editor.state.doc.descendants((node, pos) => {
		if (found >= 0) return false;
		if (node.type.name === "table") found = pos;
		return true;
	});
	return found;
}

export function isInTable(editor: Editor): boolean {
	return editor.isActive("table");
}

export function addColumn(editor: Editor): boolean {
	return editor.chain().focus().addColumnAfter().run();
}

export function addRow(editor: Editor): boolean {
	return editor.chain().focus().addRowAfter().run();
}

export function mergeSelectedCells(editor: Editor): boolean {
	return editor.chain().focus().mergeCells().run();
}

export function splitSelectedCells(editor: Editor): boolean {
	return editor.chain().focus().splitCell().run();
}

export function deleteCurrentTable(editor: Editor): boolean {
	return editor.chain().focus().deleteTable().run();
}

export function toggleTableHeaderRow(editor: Editor): boolean {
	return editor.chain().focus().toggleHeaderRow().run();
}

export function toggleTableHeaderColumn(editor: Editor): boolean {
	return editor.chain().focus().toggleHeaderColumn().run();
}

export function setCellAlign(
	editor: Editor,
	align: "left" | "center" | "right",
): boolean {
	return editor.chain().focus().setTextAlign(align).run();
}

export function setCellBackground(
	editor: Editor,
	background: string | null,
): boolean {
	return editor
		.chain()
		.focus()
		.setCellAttribute("background", background)
		.run();
}

export function equalizeColumns(editor: Editor): boolean {
	const tablePos = tablePosOf(editor);
	if (tablePos < 0) return false;
	const table = editor.state.doc.nodeAt(tablePos);
	if (!table) return false;
	const { tr } = editor.state;
	let changed = false;
	table.descendants((node, rel) => {
		if (node.type.name !== "tableCell" && node.type.name !== "tableHeader") {
			return true;
		}
		if (node.attrs.colwidth == null) return true;
		tr.setNodeMarkup(tablePos + 1 + rel, undefined, {
			...node.attrs,
			colwidth: null,
		});
		changed = true;
		return true;
	});
	if (!changed) return false;
	editor.view.dispatch(tr);
	return true;
}

export function cellPositions(editor: Editor): number[] {
	const out: number[] = [];
	editor.state.doc.descendants((node, pos) => {
		if (node.type.name === "tableCell" || node.type.name === "tableHeader") {
			out.push(pos);
		}
	});
	return out;
}

export function selectCurrentCell(editor: Editor): boolean {
	const $pos = editor.state.doc.resolve(editor.state.selection.from);
	for (let d = $pos.depth; d > 0; d--) {
		const name = $pos.node(d).type.name;
		if (name === "tableCell" || name === "tableHeader") {
			const pos = $pos.before(d);
			return editor
				.chain()
				.focus()
				.setCellSelection({ anchorCell: pos, headCell: pos })
				.run();
		}
	}
	return false;
}

function isWholeTableSelection(sel: Editor["state"]["selection"]): boolean {
	if (sel instanceof NodeSelection && sel.node.type.name === "table") {
		return true;
	}
	return (
		sel instanceof CellSelection && sel.isColSelection() && sel.isRowSelection()
	);
}

function cellsInCurrentTable(editor: Editor): number[] {
	const tablePos = tablePosOf(editor);
	if (tablePos < 0) return [];
	const table = editor.state.doc.nodeAt(tablePos);
	if (!table) return [];
	const end = tablePos + table.nodeSize;
	return cellPositions(editor).filter((pos) => pos > tablePos && pos < end);
}

export function selectWholeTable(editor: Editor): boolean {
	const pos = tablePosOf(editor);
	if (pos >= 0 && editor.chain().focus().setNodeSelection(pos).run()) {
		return true;
	}
	const cells = cellsInCurrentTable(editor);
	const first = cells[0];
	const last = cells.at(-1);
	if (first === undefined || last === undefined) return false;
	return editor
		.chain()
		.focus()
		.setCellSelection({ anchorCell: first, headCell: last })
		.run();
}

export function selectAllStep(editor: Editor): void {
	const sel = editor.state.selection;
	if (sel instanceof AllSelection) return;
	if (isWholeTableSelection(sel)) {
		remember(editor, {
			table: sel instanceof NodeSelection ? sel.from : tablePosOf(editor),
		});
		editor.chain().focus().selectAll().run();
		return;
	}
	if (sel instanceof CellSelection) {
		remember(editor, { cell: sel.$anchorCell.pos, table: tablePosOf(editor) });
		selectWholeTable(editor);
		return;
	}
	if (isInTable(editor)) {
		selectCurrentCell(editor);
		const next = editor.state.selection;
		if (next instanceof CellSelection) {
			remember(editor, {
				cell: next.$anchorCell.pos,
				table: tablePosOf(editor),
			});
		}
		return;
	}
	lastRange.delete(editor);
	editor.chain().focus().selectAll().run();
}

export function selectAllEscape(editor: Editor): boolean {
	const sel = editor.state.selection;
	const mem = lastRange.get(editor);
	if (sel instanceof AllSelection) {
		if (mem && mem.table >= 0) {
			return editor.chain().focus().setNodeSelection(mem.table).run();
		}
		return editor.chain().focus().setTextSelection(1).run();
	}
	if (sel instanceof NodeSelection && sel.node.type.name === "table") {
		if (mem && mem.cell >= 0) {
			return editor
				.chain()
				.focus()
				.setCellSelection({ anchorCell: mem.cell, headCell: mem.cell })
				.run();
		}
		return selectCurrentCell(editor);
	}
	if (sel instanceof CellSelection) {
		return editor
			.chain()
			.focus()
			.setTextSelection(sel.$anchorCell.pos + 1)
			.run();
	}
	return false;
}

export function countColumns(editor: Editor): number {
	let cols = 0;
	editor.state.doc.descendants((node) => {
		if (cols > 0) return false;
		if (node.type.name === "tableRow") {
			cols = node.childCount;
			return false;
		}
		return true;
	});
	return cols;
}
