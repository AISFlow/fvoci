import { t } from "@fvoci/i18n";
import {
	type Editor,
	Extension,
	InputRule,
	Mark,
	mergeAttributes,
	textblockTypeInputRule,
} from "@tiptap/core";
import { CodeBlockLowlight } from "@tiptap/extension-code-block-lowlight";
import { isChangeOrigin } from "@tiptap/extension-collaboration";
import {
	Details,
	DetailsContent,
	DetailsSummary,
} from "@tiptap/extension-details";
import { Emoji, type EmojiItem } from "@tiptap/extension-emoji";
import { Highlight } from "@tiptap/extension-highlight";
import { TaskItem, TaskList } from "@tiptap/extension-list";
import { NodeRange } from "@tiptap/extension-node-range";
import { TableKit } from "@tiptap/extension-table/kit";
import { TableOfContents } from "@tiptap/extension-table-of-contents";
import { TextAlign } from "@tiptap/extension-text-align";
import { Color, TextStyle } from "@tiptap/extension-text-style";
import { UniqueID } from "@tiptap/extension-unique-id";
import { Placeholder } from "@tiptap/extensions";
import { Selection } from "@tiptap/pm/state";
import { StarterKit } from "@tiptap/starter-kit";
import type { SuggestionOptions } from "@tiptap/suggestion";
import { UNIQUE_ID_NODE_TYPES } from "./extract.js";
import {
	ensureLanguage,
	HIGHLIGHT_MAX_CHARS,
	languageOfFence,
	lowlight,
	parseFenceLanguage,
	parseHighlightLines,
} from "./lowlight.js";
import { Attachment } from "./nodes/attachment.js";
import { Callout } from "./nodes/callout.js";
import { Embed } from "./nodes/embed.js";
import { MathBlock, MathInline } from "./nodes/math.js";
import { Mention } from "./nodes/mention.js";
import { Mermaid } from "./nodes/mermaid.js";

export type EmojiMenuItem = EmojiItem & { title: string };

const attemptedLangs = new Set<string>();

const YCHANGE_NODE_TYPES = [
	"heading",
	"paragraph",
	"blockquote",
	"codeBlock",
	"embed",
	"listItem",
	"table",
	"horizontalRule",
	"bulletList",
	"orderedList",
	"tableRow",
	"tableHeader",
	"tableCell",
	"callout",
	"mermaid",
	"math",
	"attachment",
	"details",
	"detailsContent",
	"detailsSummary",
	"taskList",
	"taskItem",
] as const;

function ychangeTypeOf(value: unknown): "added" | "removed" | null {
	if (typeof value !== "object" || value === null) return null;
	const type = Reflect.get(value, "type");
	return type === "added" || type === "removed" ? type : null;
}

function dataAttr(el: unknown, name: string): string | null {
	if (typeof el !== "object" || el === null) return null;
	const get = Reflect.get(el, "getAttribute");
	if (typeof get !== "function") return null;
	const v = get.call(el, name);
	return typeof v === "string" ? v : null;
}

const YChangeAttr = Extension.create({
	name: "ychangeAttr",
	addGlobalAttributes() {
		return [
			{
				types: [...YCHANGE_NODE_TYPES],
				attributes: {
					ychange: {
						default: null,
						parseHTML: (el) => {
							const type = dataAttr(el, "data-ychange-type");
							return type === "added" || type === "removed" ? { type } : null;
						},
						renderHTML: (attributes) => {
							const type = ychangeTypeOf(attributes.ychange);
							return type === null ? {} : { "data-ychange-type": type };
						},
					},
				},
			},
		];
	},
});

const YChangeMark = Mark.create({
	name: "ychange",
	addAttributes() {
		return {
			user: { default: null },
			type: {
				default: null,
				parseHTML: (el) => dataAttr(el, "data-ychange-type"),
				renderHTML: (attributes) =>
					attributes.type === "added" || attributes.type === "removed"
						? { "data-ychange-type": attributes.type }
						: {},
			},
			color: {
				default: null,
				renderHTML: () => ({}),
			},
		};
	},
	parseHTML() {
		return [{ tag: "span[data-ychange-type]" }];
	},
	renderHTML({ HTMLAttributes }) {
		return ["span", mergeAttributes(HTMLAttributes), 0];
	},
});

function emojiMenuItems({
	editor,
	query,
}: {
	editor: Editor;
	query: string;
}): EmojiMenuItem[] {
	const q = query.trim().toLowerCase();
	return editor.storage.emoji.emojis
		.filter(
			(item) =>
				item.name.toLowerCase().startsWith(q) ||
				item.shortcodes.some((code) => code.toLowerCase().startsWith(q)),
		)
		.slice(0, 8)
		.map((item) => ({
			...item,
			title: `${item.emoji ?? ""} ${item.name}`.trim(),
		}));
}

export function createFvociExtensions(opts?: {
	emojiSuggestionRender?: SuggestionOptions<EmojiMenuItem>["render"];
	emojiSuggestionFloatingUi?: SuggestionOptions<EmojiMenuItem>["floatingUi"];
}) {
	return [
		StarterKit.configure({
			undoRedo: false,
			codeBlock: false,
		}),
		CodeBlockLowlight.extend({
			addAttributes() {
				return {
					...this.parent?.(),
					highlightLines: {
						default: [],
						parseHTML: (el) =>
							parseHighlightLines(
								el.getAttribute("data-highlight-lines") ?? "",
							),
						renderHTML: (attributes) => {
							const lines = attributes.highlightLines;
							if (!Array.isArray(lines) || lines.length === 0) return {};
							return { "data-highlight-lines": lines.join(",") };
						},
					},
				};
			},
			addInputRules() {
				return [
					textblockTypeInputRule({
						find: /^```(?!mermaid)([a-z]+(?:\{[\d,\-\s]+\})?)?[\s\n]$/,
						type: this.type,
						getAttributes: (match) => {
							const parsed = parseFenceLanguage(match[1]);
							void ensureLanguage(parsed.language);
							return parsed;
						},
					}),
				];
			},
		}).configure({
			lowlight,
			enableTabIndentation: true,
			tabSize: 2,
		}),
		Extension.create({
			name: "fvociLowlightPack",
			onTransaction({ editor, transaction }) {
				/* WHY: #571 — 선택 이동·원격 awareness 트랜잭션까지 문서를 전수 순회했다.
				 * 코드블록 언어는 문서가 바뀔 때만 새로 생긴다. */
				if (!transaction.docChanged) return;
				const seen = new Set<string>();
				editor.state.doc.descendants((node) => {
					if (node.type.name !== "codeBlock") return;
					if (node.textContent.length > HIGHLIGHT_MAX_CHARS) return;
					const language = String(node.attrs.language ?? "");
					if (language) seen.add(language);
				});
				for (const language of seen) {
					const key = languageOfFence(language) || language;
					if (lowlight.listLanguages().includes(key)) continue;
					if (attemptedLangs.has(key)) continue;
					attemptedLangs.add(key);
					void ensureLanguage(key).then((resolved) => {
						if (editor.isDestroyed) return;
						if (!lowlight.listLanguages().includes(resolved)) return;
						const { tr, doc, selection, storedMarks } = editor.state;
						doc.descendants((node, pos) => {
							if (node.type.name !== "codeBlock") return;
							// WHY: Lowlight needs a node-covering step; preserve its content mapping.
							tr.setNodeMarkup(pos, undefined, node.attrs);
						});
						if (tr.steps.length === 0) return;
						// WHY: Equivalent markup refresh replaces node boundaries in the step map.
						tr.setSelection(Selection.fromJSON(tr.doc, selection.toJSON()));
						tr.setStoredMarks(storedMarks);
						tr.setMeta("addToHistory", false);
						tr.setMeta("fvociLowlight", resolved);
						editor.view.dispatch(tr);
					});
				}
			},
		}),
		TableKit.configure({ table: { resizable: true } }),
		Extension.create({
			name: "tableCellBackground",
			addGlobalAttributes() {
				return [
					{
						types: ["tableCell", "tableHeader"],
						attributes: {
							background: {
								default: null,
								parseHTML: (el) => dataAttr(el, "data-background"),
								renderHTML: (attributes) => {
									const bg = attributes.background;
									if (typeof bg !== "string" || bg.length === 0) return {};
									return {
										"data-background": bg,
										style: `background:${bg}`,
									};
								},
							},
						},
					},
				];
			},
		}),
		NodeRange,
		UniqueID.configure({
			types: [...UNIQUE_ID_NODE_TYPES],
			filterTransaction: (tr) => !isChangeOrigin(tr),
		}),
		YChangeAttr,
		YChangeMark,
		TableOfContents.configure({
			anchorTypes: ["heading"],
			...(typeof document === "undefined"
				? {}
				: {
						scrollParent: () => document.getElementById("fv-main") ?? window,
					}),
		}),
		Placeholder.configure({
			includeChildren: true,
			placeholder: () => t("editor.placeholder"),
		}),
		Highlight.configure({ multicolor: true }),
		TextStyle,
		Color,
		TextAlign.configure({ types: ["heading", "paragraph"] }),
		Details.configure({ persist: true }),
		DetailsContent,
		DetailsSummary,
		TaskList,
		TaskItem.configure({ nested: true }),
		Callout,
		Mermaid,
		Extension.create({
			name: "mermaidFence",
			addInputRules() {
				return [
					new InputRule({
						find: /^```mermaid[\s\n]$/,
						handler: ({ chain, range }) => {
							chain()
								.deleteRange(range)
								.insertContent({
									type: "mermaid",
									attrs: { source: "" },
								})
								.run();
						},
					}),
				];
			},
		}),
		MathBlock,
		MathInline,
		Attachment,
		Emoji.configure({
			suggestion: {
				items: emojiMenuItems,
				floatingUi: opts?.emojiSuggestionFloatingUi,
				...(opts?.emojiSuggestionRender
					? { render: opts.emojiSuggestionRender }
					: {}),
			},
		}),
		Mention,
		Embed,
	];
}
