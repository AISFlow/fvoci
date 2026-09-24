import { t } from "@fvoci/i18n";
import type { Editor } from "@tiptap/react";
import { useEditorState } from "@tiptap/react";
import { type ReactNode, useEffect, useState } from "react";
import {
	ensureLanguage,
	HIGHLIGHT_MAX_CHARS,
	languageOfFence,
	parseFenceLanguage,
	parseHighlightLines,
} from "../lowlight.js";
import { copyText } from "./clipboard.js";
import { Button } from "./tiptap-ui-primitive/button.js";

const LANGS = [
	"typescript",
	"javascript",
	"json",
	"css",
	"python",
	"diff",
	"",
] as const;

type ChromeState = { linenos: boolean; wrap: boolean; folded: boolean };

const DEFAULT_CHROME: ChromeState = {
	linenos: false,
	wrap: false,
	folded: false,
};

const HL_BG = {
	meta: "color-mix(in oklch, var(--ring) 32%, transparent)",
	add: "color-mix(in oklch, var(--status-done) 28%, transparent)",
	del: "color-mix(in oklch, var(--destructive) 28%, transparent)",
} as const;

function asLineNumbers(value: unknown): number[] {
	if (typeof value === "string") return parseHighlightLines(value);
	if (Array.isArray(value)) {
		return value.filter(
			(n): n is number => typeof n === "number" && Number.isInteger(n) && n > 0,
		);
	}
	return [];
}

function lineKind(
	n: number,
	row: string,
	meta: Set<number>,
	isDiff: boolean,
): keyof typeof HL_BG | "" {
	if (meta.has(n)) return "meta";
	if (!isDiff) return "";
	if (row.startsWith("+")) return "add";
	if (row.startsWith("-")) return "del";
	return "";
}

export function CodeBlockChrome({ editor }: { editor: Editor }): ReactNode {
	const block = useEditorState({
		editor,
		selector: ({ editor: current }) => {
			if (!current.isActive("codeBlock")) return null;
			const $from = current.state.selection.$from;
			const parent = $from.parent;
			const pos = $from.before($from.depth);
			const attrs = current.getAttributes("codeBlock");
			return {
				id: String(parent.attrs.id ?? pos),
				editable: current.isEditable,
				language: String(attrs.language ?? ""),
				highlightLines: attrs.highlightLines,
				text: parent.textContent,
			};
		},
	});
	const [copyFailedId, setCopyFailedId] = useState<string | null>(null);
	const [byId, setById] = useState<Record<string, ChromeState>>({});
	const chrome = block ? (byId[block.id] ?? DEFAULT_CHROME) : DEFAULT_CHROME;

	useEffect(() => {
		if (!block) return;
		const language = languageOfFence(block.language) || block.language;
		if (!language) return;
		void ensureLanguage(language, block.text);
	}, [block]);

	useEffect(() => {
		const host = editor.view.dom.closest(".fvoci-editor");
		if (!(host instanceof HTMLElement)) return;
		if (!block) {
			host.removeAttribute("data-code-folded");
			host.removeAttribute("data-code-wrap");
			return;
		}
		host.dataset.codeFolded = chrome.folded ? "true" : "false";
		host.dataset.codeWrap = chrome.wrap ? "true" : "false";
	}, [block, chrome.folded, chrome.wrap, editor]);

	if (!block) return null;
	const parsed = parseFenceLanguage(block.language);
	const language = parsed.language || block.language;
	const rows = block.text.length === 0 ? [""] : block.text.split("\n");
	const fromAttr = asLineNumbers(block.highlightLines);
	const meta = new Set(fromAttr.length > 0 ? fromAttr : parsed.highlightLines);
	const isDiff = language === "diff";
	const paint =
		block.text.length <= HIGHLIGHT_MAX_CHARS && (meta.size > 0 || isDiff);
	const patch = (next: Partial<ChromeState>) => {
		setById((prev) => ({
			...prev,
			[block.id]: { ...chrome, ...next },
		}));
	};
	return (
		<div
			className="fvoci-code-chrome"
			data-linenos={chrome.linenos ? "true" : "false"}
			data-wrap={chrome.wrap ? "true" : "false"}
			data-folded={chrome.folded ? "true" : "false"}
		>
			<div className="fvoci-format-cluster">
				<select
					aria-label={t("editor.code.language")}
					value={language}
					disabled={!block.editable}
					onChange={(event) => {
						if (editor.isDestroyed || !editor.isEditable) return;
						editor.commands.updateAttributes("codeBlock", {
							language: languageOfFence(event.target.value),
						});
					}}
				>
					{LANGS.map((lang) => (
						<option key={lang || "plain"} value={lang}>
							{lang || "plain"}
						</option>
					))}
				</select>
				<Button
					onClick={() => {
						setCopyFailedId(null);
						void copyText(block.text).catch(() => setCopyFailedId(block.id));
					}}
				>
					{t("editor.code.copy")}
				</Button>
			</div>
			{copyFailedId === block.id ? (
				<p role="alert">{t("editor.copy.failed")}</p>
			) : null}
			<div className="fvoci-format-cluster">
				<Button
					aria-pressed={chrome.linenos}
					onClick={() => patch({ linenos: !chrome.linenos })}
				>
					{t("editor.code.linenos")}
				</Button>
				{rows.length > 8 ? (
					<Button
						aria-pressed={chrome.folded}
						onClick={() => patch({ folded: !chrome.folded })}
					>
						{t("editor.code.fold")}
					</Button>
				) : null}
				<Button
					aria-pressed={chrome.wrap}
					onClick={() => patch({ wrap: !chrome.wrap })}
				>
					{t("editor.code.wrap")}
				</Button>
			</div>
			{chrome.linenos || paint ? (
				<pre className="fvoci-code-linenos" aria-hidden>
					{rows.map((row, i) => {
						const n = i + 1;
						const kind = paint ? lineKind(n, row, meta, isDiff) : "";
						return (
							<span
								key={n}
								{...(kind ? { "data-hl": kind } : {})}
								ref={(el) => {
									if (el && kind) el.style.background = HL_BG[kind];
								}}
							>
								{chrome.linenos ? String(n) : "\u00a0"}
								{"\n"}
							</span>
						);
					})}
				</pre>
			) : null}
		</div>
	);
}
