import { type I18nKey, t } from "@fvoci/i18n";
import type { Editor } from "@tiptap/react";
import { useEditorState } from "@tiptap/react";
import {
	type ReactNode,
	type PointerEvent as ReactPointerEvent,
	useCallback,
	useEffect,
	useId,
	useLayoutEffect,
	useRef,
	useState,
} from "react";
import { moveNodeTo } from "./gutter-actions.js";
import { preventSelectionLoss } from "./menu-keyboard.js";
import { PointMenu } from "./point-menu.js";
import {
	addColumn,
	addRow,
	deleteCurrentTable,
	equalizeColumns,
	isInTable,
	mergeSelectedCells,
	setCellAlign,
	setCellBackground,
	splitSelectedCells,
	tablePosOf,
	toggleTableHeaderColumn,
	toggleTableHeaderRow,
} from "./table-actions.js";
import { Button } from "./tiptap-ui-primitive/button.js";
import { Separator } from "./tiptap-ui-primitive/separator.js";

const BACKGROUNDS: Array<{ key: I18nKey; value: string | null }> = [
	{ key: "editor.color.none", value: null },
	{ key: "editor.color.muted", value: "var(--muted)" },
	{ key: "editor.color.accent", value: "var(--accent)" },
	{ key: "editor.color.danger", value: "var(--destructive)" },
];

function tableDom(editor: Editor, pos: number): HTMLElement | null {
	const dom = editor.view.nodeDOM(pos);
	if (dom instanceof HTMLElement) return dom;
	if (dom instanceof Node && dom.parentElement) return dom.parentElement;
	return null;
}

export function TableHandles({ editor }: { editor: Editor }): ReactNode {
	const insideKey = useEditorState({
		editor,
		selector: ({ editor: current }) =>
			isInTable(current)
				? `${tablePosOf(current)}:${current.state.selection.from}`
				: "",
	});
	const inside = (() => {
		if (!insideKey) return null;
		const [pos, from] = insideKey.split(":");
		return { tablePos: Number(pos), from: Number(from) };
	})();
	const [box, setBox] = useState<{
		table: DOMRect;
		left: number;
		top: number;
	} | null>(null);
	const [tick, setTick] = useState(0);
	const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
	const drag = useRef<{
		x: number;
		y: number;
		tablePos: number;
		moved: boolean;
	} | null>(null);
	const menuId = useId();
	const suppressed = useRef(false);
	const closeMenu = useCallback(() => setMenu(null), []);

	useLayoutEffect(() => {
		void tick;
		if (!inside || inside.tablePos < 0) {
			setBox(null);
			return;
		}
		const table = tableDom(editor, inside.tablePos);
		if (!table) {
			setBox(null);
			return;
		}
		const tableRect = table.getBoundingClientRect();
		// WHY: Keep coordinates local to the editor even inside a translated dialog.
		const host = editor.view.dom.closest<HTMLElement>(".fvoci-editor");
		if (!host) return;
		const origin = host.getBoundingClientRect();
		const left =
			tableRect.left - origin.left - host.clientLeft + host.scrollLeft;
		const top = tableRect.top - origin.top - host.clientTop + host.scrollTop;
		setBox((prev) => {
			if (
				prev &&
				prev.left === left &&
				prev.top === top &&
				prev.table.left === tableRect.left &&
				prev.table.top === tableRect.top &&
				prev.table.width === tableRect.width &&
				prev.table.height === tableRect.height
			) {
				return prev;
			}
			return { table: tableRect, left, top };
		});
	}, [editor, inside, tick]);

	useEffect(() => {
		const onScroll = () => setTick((n) => n + 1);
		document.addEventListener("scroll", onScroll, true);
		window.addEventListener("resize", onScroll);
		return () => {
			document.removeEventListener("scroll", onScroll, true);
			window.removeEventListener("resize", onScroll);
		};
	}, []);

	if (!inside || !box) return null;

	const { table } = box;

	const onTablePointerDown = (event: ReactPointerEvent<HTMLButtonElement>) => {
		event.preventDefault();
		event.currentTarget.focus({ preventScroll: true });
		suppressed.current = false;
		event.stopPropagation();
		drag.current = {
			x: event.clientX,
			y: event.clientY,
			tablePos: inside.tablePos,
			moved: false,
		};
		event.currentTarget.setPointerCapture(event.pointerId);
	};

	const onTablePointerMove = (event: ReactPointerEvent<HTMLButtonElement>) => {
		const start = drag.current;
		if (!start) return;
		const dx = event.clientX - start.x;
		const dy = event.clientY - start.y;
		if (dx * dx + dy * dy > 16) start.moved = true;
	};

	const onTablePointerUp = (event: ReactPointerEvent<HTMLButtonElement>) => {
		const start = drag.current;
		drag.current = null;
		if (!start) return;
		suppressed.current = start.moved;
		if (!start.moved) return;
		const hit = editor.view.posAtCoords({
			left: event.clientX,
			top: event.clientY,
		});
		if (!hit) return;
		const $pos = editor.state.doc.resolve(hit.pos);
		const insertPos = $pos.depth === 0 ? hit.pos : $pos.before(1);
		moveNodeTo(editor, start.tablePos, insertPos);
	};

	const openMenu = (event: React.MouseEvent<HTMLButtonElement>) => {
		if (
			event.currentTarget.dataset.tableHandle === "table" &&
			suppressed.current &&
			event.detail !== 0
		) {
			suppressed.current = false;
			return;
		}
		event.currentTarget.focus({ preventScroll: true });
		const rect = event.currentTarget.getBoundingClientRect();
		setMenu({ x: rect.left, y: rect.bottom });
	};

	const run = (fn: () => void) => () => {
		fn();
		setMenu(null);
	};

	return (
		<>
			<div
				className="fvoci-table-handles"
				data-table-handles=""
				style={{
					left: `${box.left}px`,
					top: `${box.top}px`,
					width: `${table.width}px`,
					height: `${table.height}px`,
				}}
			>
				<Button
					data-table-handle="table"
					aria-label={t("editor.table.handle")}
					aria-haspopup="menu"
					aria-expanded={menu !== null}
					aria-controls={menu ? menuId : undefined}
					style={{ left: "0", top: "-2.75rem" }}
					onPointerDown={onTablePointerDown}
					onPointerMove={onTablePointerMove}
					onPointerUp={onTablePointerUp}
					onPointerCancel={() => {
						drag.current = null;
						suppressed.current = true;
					}}
					onClick={openMenu}
				>
					{t("editor.block.table")}
				</Button>
				<Button
					data-table-handle="col"
					aria-label={t("editor.table.colHandle")}
					aria-haspopup="menu"
					aria-expanded={menu !== null}
					aria-controls={menu ? menuId : undefined}
					style={{ left: "2.75rem", top: "-2.75rem" }}
					onClick={openMenu}
				>
					↕
				</Button>
				<Button
					data-table-handle="row"
					aria-label={t("editor.table.rowHandle")}
					aria-haspopup="menu"
					aria-expanded={menu !== null}
					aria-controls={menu ? menuId : undefined}
					style={{ left: "5.5rem", top: "-2.75rem" }}
					onClick={openMenu}
				>
					↔
				</Button>
				<Button
					data-table-handle="col-plus"
					aria-label={t("editor.table.addCol")}
					style={{ left: "8.25rem", top: "-2.75rem" }}
					onClick={() => addColumn(editor)}
				>
					+
				</Button>
				<Button
					data-table-handle="row-plus"
					aria-label={t("editor.table.addRow")}
					style={{ left: "11rem", top: "-2.75rem" }}
					onClick={() => addRow(editor)}
				>
					+
				</Button>
			</div>
			{menu ? (
				<PointMenu
					x={menu.x}
					y={menu.y}
					owner={editor.view.dom}
					onClose={closeMenu}
					label={t("editor.block.table")}
					id={menuId}
				>
					<div className="fvoci-block-menu__group">
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => addColumn(editor))}
						>
							{t("editor.table.insertCol")}
						</button>
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => editor.chain().focus().deleteColumn().run())}
						>
							{t("editor.table.deleteCol")}
						</button>
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => addRow(editor))}
						>
							{t("editor.table.insertRow")}
						</button>
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => editor.chain().focus().deleteRow().run())}
						>
							{t("editor.table.deleteRow")}
						</button>
					</div>
					<Separator />
					<div className="fvoci-block-menu__group">
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() =>
								moveNodeTo(
									editor,
									inside.tablePos,
									Math.max(0, inside.tablePos - 1),
								),
							)}
						>
							{t("editor.table.moveUp")}
						</button>
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => {
								const node = editor.state.doc.nodeAt(inside.tablePos);
								if (!node) return;
								moveNodeTo(
									editor,
									inside.tablePos,
									inside.tablePos + node.nodeSize,
								);
							})}
						>
							{t("editor.table.moveDown")}
						</button>
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => toggleTableHeaderRow(editor))}
						>
							{t("editor.table.headerRow")}
						</button>
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => toggleTableHeaderColumn(editor))}
						>
							{t("editor.table.headerCol")}
						</button>
					</div>
					<Separator />
					<div className="fvoci-block-menu__group">
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => setCellAlign(editor, "left"))}
						>
							{t("editor.align.left")}
						</button>
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => setCellAlign(editor, "center"))}
						>
							{t("editor.align.center")}
						</button>
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => setCellAlign(editor, "right"))}
						>
							{t("editor.align.right")}
						</button>
						{BACKGROUNDS.map((item) => (
							<button
								key={item.key}
								type="button"
								role="menuitem"
								onMouseDown={preventSelectionLoss}
								onClick={run(() => setCellBackground(editor, item.value))}
							>
								{t("editor.table.background", { name: t(item.key) })}
							</button>
						))}
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => mergeSelectedCells(editor))}
						>
							{t("editor.table.merge")}
						</button>
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => splitSelectedCells(editor))}
						>
							{t("editor.table.split")}
						</button>
						<button
							type="button"
							role="menuitem"
							onMouseDown={preventSelectionLoss}
							onClick={run(() => equalizeColumns(editor))}
						>
							{t("editor.table.equalize")}
						</button>
					</div>
					<Separator />
					<button
						type="button"
						role="menuitem"
						onMouseDown={preventSelectionLoss}
						onClick={run(() => deleteCurrentTable(editor))}
					>
						{t("editor.table.delete")}
					</button>
				</PointMenu>
			) : null}
		</>
	);
}
