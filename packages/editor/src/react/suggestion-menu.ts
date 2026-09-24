import { shift, size } from "@floating-ui/dom";
import { t } from "@fvoci/i18n";
import type { Editor, Range } from "@tiptap/core";
import type {
	SuggestionKeyDownProps,
	SuggestionProps,
} from "@tiptap/suggestion";
import { overlayOwner } from "./overlay-owner.js";

export type SlashItem = {
	title: string;
	aliases: string[];
	run: (editor: Editor, range: Range) => void;
};

function slashItems(): SlashItem[] {
	return [
		{
			title: t("editor.block.heading1"),
			aliases: ["h1", "heading"],
			run: (editor, range) => {
				editor
					.chain()
					.focus()
					.deleteRange(range)
					.setHeading({ level: 1 })
					.run();
			},
		},
		{
			title: t("editor.block.heading2"),
			aliases: ["h2"],
			run: (editor, range) => {
				editor
					.chain()
					.focus()
					.deleteRange(range)
					.setHeading({ level: 2 })
					.run();
			},
		},
		{
			title: t("editor.block.heading3"),
			aliases: ["h3"],
			run: (editor, range) => {
				editor
					.chain()
					.focus()
					.deleteRange(range)
					.setHeading({ level: 3 })
					.run();
			},
		},
		{
			title: t("editor.block.paragraph"),
			aliases: ["p", "paragraph"],
			run: (editor, range) => {
				editor.chain().focus().deleteRange(range).setParagraph().run();
			},
		},
		{
			title: t("editor.block.bullet"),
			aliases: ["ul", "bullet"],
			run: (editor, range) => {
				editor.chain().focus().deleteRange(range).toggleBulletList().run();
			},
		},
		{
			title: t("editor.block.ordered"),
			aliases: ["ol", "ordered"],
			run: (editor, range) => {
				editor.chain().focus().deleteRange(range).toggleOrderedList().run();
			},
		},
		{
			title: t("editor.block.blockquote"),
			aliases: ["quote"],
			run: (editor, range) => {
				editor.chain().focus().deleteRange(range).toggleBlockquote().run();
			},
		},
		{
			title: t("editor.mark.code"),
			aliases: ["code"],
			run: (editor, range) => {
				editor.chain().focus().deleteRange(range).toggleCodeBlock().run();
			},
		},
		{
			title: t("editor.block.table"),
			aliases: ["table"],
			run: (editor, range) => {
				editor
					.chain()
					.focus()
					.deleteRange(range)
					.insertTable({ rows: 2, cols: 2, withHeaderRow: true })
					.run();
			},
		},
		{
			title: t("editor.block.hr"),
			aliases: ["hr"],
			run: (editor, range) => {
				editor.chain().focus().deleteRange(range).setHorizontalRule().run();
			},
		},
		{
			title: t("editor.block.task"),
			aliases: ["todo", "task", "checklist"],
			run: (editor, range) => {
				editor.chain().focus().deleteRange(range).toggleTaskList().run();
			},
		},
		{
			title: t("editor.block.callout"),
			aliases: ["callout", "note"],
			run: (editor, range) => {
				editor
					.chain()
					.focus()
					.deleteRange(range)
					.insertContent({
						type: "callout",
						attrs: { kind: "note" },
						content: [{ type: "paragraph" }],
					})
					.run();
			},
		},
		{
			title: t("editor.block.toggle"),
			aliases: ["details", "toggle"],
			run: (editor, range) => {
				editor.chain().focus().deleteRange(range).setDetails().run();
			},
		},
		{
			title: t("editor.block.math"),
			aliases: ["math", "latex"],
			run: (editor, range) => {
				editor
					.chain()
					.focus()
					.deleteRange(range)
					.insertContent({ type: "math", attrs: { latex: "" } })
					.run();
			},
		},
		{
			title: "mermaid",
			aliases: ["mermaid"],
			run: (editor, range) => {
				editor
					.chain()
					.focus()
					.deleteRange(range)
					.insertContent({
						type: "mermaid",
						attrs: { source: "" },
					})
					.run();
			},
		},
		{
			title: t("editor.block.attachment"),
			aliases: ["attachment", "file"],
			run: (editor, range) => {
				editor
					.chain()
					.focus()
					.deleteRange(range)
					.insertContent({
						type: "attachment",
						attrs: { name: "", image: false },
					})
					.run();
			},
		},
	];
}

export function filterSlashItems(query: string): SlashItem[] {
	const items = slashItems();
	const q = query.trim().toLowerCase();
	if (q.length === 0) return items;
	return items.filter(
		(item) =>
			item.title.toLowerCase().includes(q) ||
			item.aliases.some((alias) => alias.includes(q)),
	);
}

const URL_RE = /^https?:\/\/\S+$/i;

export function embedSlashItems(
	query: string,
	hits: ReadonlyArray<{ entity: string; id: string; label: string }>,
	selectedText = "",
): SlashItem[] {
	const items: SlashItem[] = [];
	const trimmed = query.trim();
	const selected = selectedText.trim();
	const url = URL_RE.test(trimmed)
		? trimmed
		: URL_RE.test(selected)
			? selected
			: "";
	if (url.length > 0) {
		items.push({
			title: t("editor.slash.embedUrl"),
			aliases: ["url", "embed"],
			run: (editor, range) => {
				editor
					.chain()
					.focus()
					.deleteRange(range)
					.insertContent({
						type: "embed",
						attrs: { entity: "url", ref: url },
					})
					.run();
			},
		});
	}
	for (const hit of hits) {
		if (hit.entity !== "document" && hit.entity !== "task") continue;
		items.push({
			title: t("editor.slash.embed", { label: hit.label }),
			aliases: ["embed"],
			run: (editor, range) => {
				editor
					.chain()
					.focus()
					.deleteRange(range)
					.insertContent({
						type: "embed",
						attrs: { entity: hit.entity, ref: hit.id },
					})
					.run();
			},
		});
	}
	return items;
}

export const suggestionFloatingUi = {
	strategy: "fixed" as const,
	middleware: [
		shift({ padding: 8 }),
		size({
			padding: 8,
			apply({ availableHeight, availableWidth, elements }) {
				elements.floating.style.maxHeight = `min(50dvh, ${Math.max(0, availableHeight)}px)`;
				elements.floating.style.maxWidth = `min(22rem, ${Math.max(0, availableWidth)}px)`;
			},
		}),
	],
};

export type MenuItem = { title: string };

/* WHY: #628 — @tiptap/suggestion 은 items() 가 끝날 때까지 쿼리와 무관한 initialItems 만
 * 재발행한다. 네트워크가 필요 없는 정적 항목은 렌더러가 쿼리에서 직접 만들어 먼저 그리고,
 * 조회 결과(props.items)는 도착하는 대로 뒤에 붙인다. */
export function suggestionRenderer<T extends MenuItem>(
	staticItems?: (query: string) => T[],
): {
	onStart: (props: SuggestionProps<T, T>) => void;
	onUpdate: (props: SuggestionProps<T, T>) => void;
	onKeyDown: (props: SuggestionKeyDownProps) => boolean;
	onExit: () => void;
} {
	let root: HTMLElement | null = null;
	let unmount: (() => void) | undefined;
	let selected = 0;
	let items: T[] = [];
	let query: string | null = null;
	let latest: SuggestionProps<T, T> | null = null;
	let editorDom: HTMLElement | null = null;
	let oldAttributes: Array<[string, string | null]> = [];
	const onEscape = (event: KeyboardEvent) => {
		if (
			event.key !== "Escape" ||
			event.isComposing ||
			!latest ||
			(event.target !== editorDom && event.target !== root)
		)
			return;
		// WHY: modal Radix listens on document capture before ProseMirror; give its focused suggestion first refusal.
		const editor = latest.editor;
		const view = editor.view;
		const fromMenu = event.target === root;
		if (view.composing) return;
		if (view.someProp("handleKeyDown", (handler) => handler(view, event))) {
			event.preventDefault();
			event.stopPropagation();
			if (fromMenu) editor.commands.focus();
		}
	};

	const merge = (props: SuggestionProps<T, T>): T[] => [
		...(staticItems?.(props.query) ?? []),
		...props.items,
	];

	/* WHY: #628 — 빈 목록에서도 커서는 0 아래로 못 내려간다. 음수가 살아남으면 조회가
	 * 도착해도 선택된 항목이 없어 Enter 가 문단을 가른다. */
	const clamp = (index: number): number =>
		Math.max(0, Math.min(index, items.length - 1));

	const paint = (): void => {
		if (!root) return;
		root.replaceChildren();
		root.setAttribute("role", "listbox");
		root.tabIndex = 0;
		root.setAttribute("aria-label", t("editor.slash.aria"));
		root.className = "fvoci-suggestion";
		items.forEach((item, i) => {
			const btn = document.createElement("button");
			btn.type = "button";
			btn.className = "fvoci-ui-button";
			btn.setAttribute("role", "option");
			btn.id = `${root?.id}-option-${i}`;
			btn.tabIndex = -1;
			btn.setAttribute("aria-selected", String(i === selected));
			const emoji =
				"emoji" in item && typeof item.emoji === "string" ? item.emoji : "";
			if (emoji.length > 0) {
				const mark = document.createElement("span");
				mark.className = "fvoci-suggestion-emoji";
				mark.textContent = emoji;
				const label = document.createElement("span");
				label.textContent =
					"name" in item && typeof item.name === "string"
						? item.name
						: item.title;
				btn.append(mark, label);
			} else {
				btn.textContent = item.title;
			}
			btn.addEventListener("mousedown", (event) => {
				event.preventDefault();
			});
			btn.addEventListener("click", () => latest?.command(item));
			root?.append(btn);
		});
		const active = root.children[selected];
		if (active instanceof HTMLElement) {
			editorDom?.setAttribute("aria-activedescendant", active.id);
			root.scrollTop = Math.max(
				active.offsetTop + active.offsetHeight - root.clientHeight,
				Math.min(root.scrollTop, active.offsetTop),
			);
		} else editorDom?.removeAttribute("aria-activedescendant");
	};

	return {
		onStart(props) {
			latest = props;
			items = merge(props);
			query = props.query;
			selected = 0;
			editorDom = props.editor.view.dom;
			root = editorDom.ownerDocument.createElement("div");
			root.id = `fvoci-suggestion-${crypto.randomUUID()}`;
			root.addEventListener("keydown", (event) => {
				if (event.target !== root || !latest) return;
				const editor = latest.editor;
				if (
					editor.view.someProp("handleKeyDown", (handler) =>
						handler(editor.view, event),
					)
				) {
					event.preventDefault();
					editor.commands.focus();
				}
			});
			oldAttributes = [
				"aria-controls",
				"aria-activedescendant",
				"aria-haspopup",
				"aria-autocomplete",
			].map((name) => [name, editorDom?.getAttribute(name) ?? null]);
			editorDom.setAttribute("aria-controls", root.id);
			editorDom.setAttribute("aria-haspopup", "listbox");
			editorDom.setAttribute("aria-autocomplete", "list");
			overlayOwner(editorDom).append(root);
			unmount = props.mount(root);
			editorDom.ownerDocument.defaultView?.addEventListener(
				"keydown",
				onEscape,
				true,
			);
			paint();
		},
		onUpdate(props) {
			latest = props;
			items = merge(props);
			/* WHY: #628 — 로딩 중에도 목록이 살아 있어 커서를 움직일 수 있다.
			 * 같은 쿼리의 조회 결과가 뒤늦게 붙었다고 커서를 맨 위로 되돌리지 않는다. */
			if (props.query !== query) {
				query = props.query;
				selected = 0;
			} else selected = clamp(selected);
			paint();
		},
		onKeyDown({ event, view }) {
			if (event.isComposing || view.composing || event.keyCode === 229)
				return false;
			if (event.key === "ArrowDown") {
				selected = clamp(selected + 1);
				paint();
				return true;
			}
			if (event.key === "ArrowUp") {
				selected = clamp(selected - 1);
				paint();
				return true;
			}
			if (event.key === "Enter") {
				const item = items[selected];
				/* WHY: #625 — 조회 로딩 중 빈 목록은 Enter 를 삼키고, 진짜 0건일 때만 에디터의 줄바꿈으로 넘긴다. */
				if (!item) return latest?.loading === true;
				latest?.command(item);
				return true;
			}
			/* WHY: #654 — Escape 는 @tiptap/suggestion 이 직접 처리한다(handleKeyDown → dispatchExit).
			 * 여기서 먼저 걷으면 뒤따르는 onExit 이 한 번 더 걷고, 상태를 끊는 것은 그 exit 트랜잭션이다. */
			return false;
		},
		onExit() {
			unmount?.();
			unmount = undefined;
			root?.remove();
			editorDom?.ownerDocument.defaultView?.removeEventListener(
				"keydown",
				onEscape,
				true,
			);
			for (const [name, value] of oldAttributes) {
				if (value === null) editorDom?.removeAttribute(name);
				else editorDom?.setAttribute(name, value);
			}
			oldAttributes = [];
			editorDom = null;
			root = null;
			latest = null;
		},
	};
}
