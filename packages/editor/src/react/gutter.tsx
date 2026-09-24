import { t } from "@fvoci/i18n";
import { DragHandle } from "@tiptap/extension-drag-handle-react";
import type { Node } from "@tiptap/pm/model";
import { type Editor, useEditorState } from "@tiptap/react";
import { type ReactNode, useEffect, useId, useRef, useState } from "react";
import { BlockMenu } from "./block-menu.js";
import { isInsideTable, isTableBlock, plusAt } from "./gutter-actions.js";

export function Gutter({
	editor,
	addLabel = t("editor.gutter.add"),
	moveLabel = t("editor.gutter.move"),
}: {
	editor: Editor;
	addLabel?: string;
	moveLabel?: string;
}): ReactNode {
	const keyboardPos = useEditorState({
		editor,
		selector: ({ editor: current }) => {
			const $pos = current.state.selection.$from;
			const pos = $pos.depth ? $pos.before($pos.depth) : $pos.pos;
			return isInsideTable(current, pos) ||
				isTableBlock(current.state.doc.nodeAt(pos))
				? -1
				: pos;
		},
	});
	const [block, setBlock] = useState<{ node: Node | null; pos: number }>({
		node: null,
		pos: -1,
	});
	const [menu, setMenu] = useState<{
		x: number;
		y: number;
		pos: number;
	} | null>(null);
	const [touch, setTouch] = useState(false);
	const menuId = useId();
	const suppressed = useRef(false);
	const press = useRef<{ x: number; y: number } | null>(null);

	useEffect(() => {
		const onContext = (event: MouseEvent) => {
			if (!editor.isEditable) return;
			if (!editor.state.selection.empty) return;
			const pos = editor.view.posAtCoords({
				left: event.clientX,
				top: event.clientY,
			});
			if (pos === null) return;
			const $pos = editor.state.doc.resolve(pos.pos);
			const depth = $pos.depth;
			if (depth === 0) return;
			const blockPos = $pos.before(depth);
			if (isInsideTable(editor, blockPos)) return;
			event.preventDefault();
			setBlock({ node: editor.state.doc.nodeAt(blockPos), pos: blockPos });
			setMenu({ x: event.clientX, y: event.clientY, pos: blockPos });
		};
		const dom = editor.view.dom;
		dom.addEventListener("contextmenu", onContext);
		let hold = 0;
		const onPointerDown = (event: PointerEvent) => {
			if (event.pointerType !== "touch") return;
			hold = window.setTimeout(() => setTouch(true), 500);
		};
		const onPointerUp = () => {
			window.clearTimeout(hold);
			setTouch(false);
		};
		dom.addEventListener("pointerdown", onPointerDown);
		document.addEventListener("pointerup", onPointerUp);
		return () => {
			dom.removeEventListener("contextmenu", onContext);
			dom.removeEventListener("pointerdown", onPointerDown);
			document.removeEventListener("pointerup", onPointerUp);
			window.clearTimeout(hold);
		};
	}, [editor]);

	const inTable =
		isTableBlock(block.node) ||
		(block.pos >= 0 && isInsideTable(editor, block.pos));

	return (
		<>
			<button
				type="button"
				className="fvoci-gutter-keyboard"
				disabled={keyboardPos < 0}
				aria-label={moveLabel}
				aria-haspopup="menu"
				aria-expanded={menu !== null}
				aria-controls={menu ? menuId : undefined}
				onClick={(event) => {
					const pos = keyboardPos;
					if (pos < 0) return;
					const rect = event.currentTarget.getBoundingClientRect();
					setBlock({ node: editor.state.doc.nodeAt(pos), pos });
					setMenu({ x: rect.left, y: rect.bottom, pos });
				}}
			>
				⠿
			</button>
			<DragHandle
				editor={editor}
				nested
				className={[
					"fvoci-gutter",
					inTable ? "fvoci-gutter-hidden" : "",
					touch ? "fvoci-gutter-touch" : "",
				]
					.filter(Boolean)
					.join(" ")}
				onNodeChange={({ node, pos }) => {
					setBlock({ node, pos });
				}}
			>
				<button
					type="button"
					data-gutter="plus"
					aria-label={addLabel}
					onPointerDown={(event) => {
						event.stopPropagation();
						const handle = event.currentTarget.closest(".fvoci-gutter");
						if (handle instanceof HTMLElement) handle.draggable = false;
					}}
					onPointerUp={(event) => {
						const handle = event.currentTarget.closest(".fvoci-gutter");
						if (handle instanceof HTMLElement) handle.draggable = true;
					}}
					onClick={(event) => {
						event.preventDefault();
						if (block.pos < 0) return;
						plusAt(editor, block.pos);
					}}
				>
					+
				</button>
				<button
					type="button"
					data-gutter="drag"
					aria-label={moveLabel}
					aria-haspopup="menu"
					aria-expanded={menu !== null}
					aria-controls={menu ? menuId : undefined}
					onPointerDown={(event) => {
						suppressed.current = false;
						press.current = { x: event.clientX, y: event.clientY };
					}}
					onPointerMove={(event) => {
						const start = press.current;
						if (
							start &&
							(event.clientX - start.x) ** 2 + (event.clientY - start.y) ** 2 >
								16
						)
							suppressed.current = true;
					}}
					onDragStart={() => {
						suppressed.current = true;
					}}
					onPointerCancel={() => {
						suppressed.current = true;
						press.current = null;
					}}
					onClick={(event) => {
						press.current = null;
						if (suppressed.current && event.detail !== 0) {
							suppressed.current = false;
							return;
						}
						if (block.pos < 0) return;
						event.currentTarget.focus({ preventScroll: true });
						const rect = event.currentTarget.getBoundingClientRect();
						setMenu({ x: rect.left, y: rect.bottom, pos: block.pos });
					}}
				>
					⠿
				</button>
			</DragHandle>
			{menu && menu.pos >= 0 ? (
				<BlockMenu
					id={menuId}
					editor={editor}
					pos={menu.pos}
					x={menu.x}
					y={menu.y}
					onClose={() => setMenu(null)}
				/>
			) : null}
		</>
	);
}
