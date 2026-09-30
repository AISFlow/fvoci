import type { Editor, JSONContent } from "@tiptap/core";
import type { Node } from "@tiptap/pm/model";

const TABLE_TYPES = new Set(["table", "tableRow", "tableCell", "tableHeader"]);

export function isTableBlock(node: Node | null): boolean {
	return node !== null && TABLE_TYPES.has(node.type.name);
}

export function isInsideTable(editor: Editor, pos: number): boolean {
	const size = editor.state.doc.content.size;
	if (pos < 0 || pos > size) return false;
	const $pos = editor.state.doc.resolve(Math.min(pos, size));
	for (let d = $pos.depth; d > 0; d--) {
		if (TABLE_TYPES.has($pos.node(d).type.name)) return true;
	}
	return false;
}

export function insertSlashHere(editor: Editor): boolean {
	const $pos = editor.state.selection.$from;
	if ($pos.depth === 0) {
		return editor.chain().focus().insertContent("/").run();
	}
	return plusAt(editor, $pos.before($pos.depth));
}

export function plusAt(editor: Editor, pos: number): boolean {
	const node = editor.state.doc.nodeAt(pos);
	if (!node) return false;
	if (node.textContent.trim().length === 0) {
		return editor
			.chain()
			.focus()
			.setTextSelection(pos + 1)
			.insertContent("/")
			.run();
	}
	const after = pos + node.nodeSize;
	return editor
		.chain()
		.focus()
		.insertContentAt(after, { type: "paragraph" })
		.setTextSelection(after + 1)
		.insertContent("/")
		.run();
}

function innerBlocks(node: Node): JSONContent[] {
	if (node.type.name === "listItem" || node.type.name === "taskItem") {
		const json = node.toJSON();
		return Array.isArray(json.content) ? json.content : [];
	}
	if (
		node.type.name === "bulletList" ||
		node.type.name === "orderedList" ||
		node.type.name === "taskList"
	) {
		const out: JSONContent[] = [];
		node.forEach((child) => {
			out.push(...innerBlocks(child));
		});
		return out.length > 0 ? out : [{ type: "paragraph" }];
	}
	return [node.toJSON()];
}

function insertCallout(
	editor: Editor,
	from: number,
	size: number,
	content: JSONContent[],
): boolean {
	return editor
		.chain()
		.focus()
		.deleteRange({ from: from, to: from + size })
		.insertContentAt(from, {
			type: "callout",
			attrs: { kind: "note" },
			content,
		})
		.setTextSelection(from + 2)
		.run();
}

export function convertToCallout(editor: Editor, pos: number): boolean {
	const node = editor.state.doc.nodeAt(pos);
	if (!node || node.type.name === "callout") return false;
	const $pos = editor.state.doc.resolve(pos);
	const content = innerBlocks(node);
	if (node.type.name === "listItem" || node.type.name === "taskItem") {
		const parent = $pos.parent;
		if (
			parent.type.name === "bulletList" ||
			parent.type.name === "orderedList" ||
			parent.type.name === "taskList"
		) {
			const listPos = $pos.before($pos.depth);
			const index = $pos.index();
			if (parent.childCount === 1) {
				return insertCallout(editor, listPos, parent.nodeSize, content);
			}
			const before: JSONContent[] = [];
			const after: JSONContent[] = [];
			let prefix = 0;
			parent.forEach((child, _offset, i) => {
				if (i < index) {
					before.push(child.toJSON());
					prefix += child.nodeSize;
				} else if (i > index) {
					after.push(child.toJSON());
				}
			});
			const replacement: JSONContent[] = [];
			if (before.length > 0) {
				replacement.push({ type: parent.type.name, content: before });
				prefix += 2;
			}
			replacement.push({
				type: "callout",
				attrs: { kind: "note" },
				content,
			});
			if (after.length > 0) {
				replacement.push({ type: parent.type.name, content: after });
			}
			return editor
				.chain()
				.focus()
				.deleteRange({ from: listPos, to: listPos + parent.nodeSize })
				.insertContentAt(listPos, replacement)
				.setTextSelection(listPos + prefix + 2)
				.run();
		}
	}
	return insertCallout(editor, pos, node.nodeSize, content);
}

export function convertBlock(
	editor: Editor,
	pos: number,
	kind:
		| "paragraph"
		| "heading1"
		| "heading2"
		| "heading3"
		| "blockquote"
		| "bulletList"
		| "orderedList"
		| "taskList"
		| "codeBlock"
		| "callout"
		| "details",
): boolean {
	const node = editor.state.doc.nodeAt(pos);
	if (!node) return false;
	if (kind === "callout") return convertToCallout(editor, pos);
	editor.chain().focus().setNodeSelection(pos).run();
	switch (kind) {
		case "paragraph":
			return editor.chain().focus().setParagraph().run();
		case "heading1":
			return editor.chain().focus().setHeading({ level: 1 }).run();
		case "heading2":
			return editor.chain().focus().setHeading({ level: 2 }).run();
		case "heading3":
			return editor.chain().focus().setHeading({ level: 3 }).run();
		case "blockquote":
			return editor.chain().focus().toggleBlockquote().run();
		case "bulletList":
			return editor.chain().focus().toggleBulletList().run();
		case "orderedList":
			return editor.chain().focus().toggleOrderedList().run();
		case "taskList":
			return editor.chain().focus().toggleTaskList().run();
		case "codeBlock":
			return editor.chain().focus().toggleCodeBlock().run();
		case "details":
			return editor.chain().focus().setDetails().run();
	}
}

export function moveNodeTo(
	editor: Editor,
	from: number,
	insertPos: number,
): boolean {
	const node = editor.state.doc.nodeAt(from);
	if (!node) return false;
	const size = node.nodeSize;
	if (insertPos > from && insertPos < from + size) return false;
	const mapped = insertPos > from ? insertPos - size : insertPos;
	return editor
		.chain()
		.deleteRange({ from, to: from + size })
		.insertContentAt(mapped, node.toJSON())
		.run();
}

export function moveNodeAfter(
	editor: Editor,
	from: number,
	after: number,
): boolean {
	const target = editor.state.doc.nodeAt(after);
	if (!target) return false;
	return moveNodeTo(editor, from, after + target.nodeSize);
}

export function moveBlock(editor: Editor, pos: number, dir: -1 | 1): boolean {
	const node = editor.state.doc.nodeAt(pos);
	if (!node) return false;
	const $pos = editor.state.doc.resolve(pos);
	const index = $pos.index();
	const parent = $pos.parent;
	const target = index + dir;
	if (target < 0 || target >= parent.childCount) return false;
	if (dir === 1) return moveNodeAfter(editor, pos, pos + node.nodeSize);
	const other = parent.child(target);
	return moveNodeTo(editor, pos, pos - other.nodeSize);
}

export function duplicateBlock(editor: Editor, pos: number): boolean {
	const node = editor.state.doc.nodeAt(pos);
	if (!node) return false;
	return editor
		.chain()
		.insertContentAt(pos + node.nodeSize, node.toJSON())
		.run();
}

export function deleteBlock(editor: Editor, pos: number): boolean {
	const node = editor.state.doc.nodeAt(pos);
	if (!node) return false;
	return editor
		.chain()
		.deleteRange({ from: pos, to: pos + node.nodeSize })
		.run();
}

export function blockTexts(editor: Editor): string[] {
	const out: string[] = [];
	editor.state.doc.forEach((node) => {
		out.push(node.textContent);
	});
	return out;
}
