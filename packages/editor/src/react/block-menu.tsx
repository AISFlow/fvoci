import { type I18nKey, t } from "@fvoci/i18n";
import type { Editor } from "@tiptap/core";
import { type ReactNode, useState } from "react";
import { copyText } from "./clipboard.js";
import {
	convertBlock,
	deleteBlock,
	duplicateBlock,
	moveBlock,
} from "./gutter-actions.js";
import { preventSelectionLoss } from "./menu-keyboard.js";
import { PointMenu } from "./point-menu.js";
import { Separator } from "./tiptap-ui-primitive/separator.js";

const CONVERT: Array<{
	key: I18nKey;
	kind: Parameters<typeof convertBlock>[2];
}> = [
	{ key: "editor.block.paragraph", kind: "paragraph" },
	{ key: "editor.block.heading1", kind: "heading1" },
	{ key: "editor.block.heading2", kind: "heading2" },
	{ key: "editor.block.heading3", kind: "heading3" },
	{ key: "editor.block.blockquote", kind: "blockquote" },
	{ key: "editor.block.bullet", kind: "bulletList" },
	{ key: "editor.block.ordered", kind: "orderedList" },
	{ key: "editor.block.task", kind: "taskList" },
	{ key: "editor.mark.code", kind: "codeBlock" },
	{ key: "editor.block.callout", kind: "callout" },
	{ key: "editor.block.toggle", kind: "details" },
];

export function BlockMenu({
	editor,
	pos,
	x,
	y,
	onClose,
	id,
}: {
	editor: Editor;
	pos: number;
	x: number;
	y: number;
	onClose: () => void;
	id?: string;
}): ReactNode {
	const [copyFailed, setCopyFailed] = useState(false);
	const run = (fn: () => void) => () => {
		fn();
		onClose();
	};
	const node = editor.state.doc.nodeAt(pos);
	const blockId = typeof node?.attrs.id === "string" ? node.attrs.id : "";
	return (
		<PointMenu
			x={x}
			y={y}
			owner={editor.view.dom}
			onClose={onClose}
			label={t("editor.menu.block")}
			id={id}
		>
			<div className="fvoci-block-menu__group">
				{CONVERT.map((item) => (
					<button
						key={item.kind}
						type="button"
						role="menuitem"
						onMouseDown={preventSelectionLoss}
						onClick={run(() => {
							convertBlock(editor, pos, item.kind);
						})}
					>
						{t(item.key)}
					</button>
				))}
			</div>
			<Separator />
			<div className="fvoci-block-menu__group">
				<button
					type="button"
					role="menuitem"
					onMouseDown={preventSelectionLoss}
					onClick={run(() => {
						duplicateBlock(editor, pos);
					})}
				>
					{t("editor.menu.duplicate")}
				</button>
				<button
					type="button"
					role="menuitem"
					onMouseDown={preventSelectionLoss}
					onClick={run(() => {
						moveBlock(editor, pos, -1);
					})}
				>
					{t("editor.menu.up")}
				</button>
				<button
					type="button"
					role="menuitem"
					onMouseDown={preventSelectionLoss}
					onClick={run(() => {
						moveBlock(editor, pos, 1);
					})}
				>
					{t("editor.menu.down")}
				</button>
				<button
					type="button"
					role="menuitem"
					onMouseDown={preventSelectionLoss}
					onClick={run(() => {
						const node = editor.state.doc.nodeAt(pos);
						if (!node) return;
						editor
							.chain()
							.focus()
							.setTextSelection({
								from: pos + 1,
								to: pos + node.nodeSize - 1,
							})
							.setColor("var(--destructive)")
							.run();
					})}
				>
					{t("editor.color")}
				</button>
				<button
					type="button"
					role="menuitem"
					onMouseDown={preventSelectionLoss}
					onClick={() => {
						if (blockId.length === 0) return;
						setCopyFailed(false);
						void copyText(`#${blockId}`).then(onClose, () =>
							setCopyFailed(true),
						);
					}}
				>
					{t("editor.menu.copyLink")}
				</button>
			</div>
			{copyFailed ? <p role="alert">{t("editor.copy.failed")}</p> : null}
			<Separator />
			<button
				type="button"
				role="menuitem"
				onMouseDown={preventSelectionLoss}
				onClick={run(() => {
					deleteBlock(editor, pos);
				})}
			>
				{t("editor.menu.delete")}
			</button>
		</PointMenu>
	);
}
