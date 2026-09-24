import { uuid } from "./uuid.js";
import { emojiGlyph } from "./emoji-glyph.js";
import type { TiptapDoc } from "./json.js";

/*
 * WHY: 가드를 extractText 진입에만 두면 자식 content 재귀가 그대로 스택을 먹는다.
 * 한도는 재귀 함수 안에서 depth+1 로 넘겨 중첩마다 센다.
 */
const TIPTAP_WALK_MAX_DEPTH = 64;

function str(v: unknown): string {
	return typeof v === "string" ? v : "";
}

function tiptapText(node: unknown, depth = 0): string {
	if (depth > TIPTAP_WALK_MAX_DEPTH) return "";
	if (typeof node === "string") return node;
	if (typeof node !== "object" || node === null) return "";
	const n = node as {
		type?: unknown;
		text?: unknown;
		attrs?: Record<string, unknown>;
		content?: unknown[];
	};
	if (typeof n.text === "string") return n.text;
	if (n.type === "mention") {
		const label = str(n.attrs?.label);
		return label.length > 0 ? label : str(n.attrs?.id);
	}
	if (n.type === "embed") return str(n.attrs?.ref);
	if (n.type === "mermaid") return str(n.attrs?.source);
	if (n.type === "math" || n.type === "mathInline") return str(n.attrs?.latex);
	if (n.type === "attachment") return str(n.attrs?.name);
	if (n.type === "hardBreak") return "\n";
	if (n.type === "emoji") return emojiGlyph(n);
	if (!Array.isArray(n.content)) return "";
	if (n.type === "table") {
		return n.content
			.map((row) => {
				const cells =
					typeof row === "object" &&
					row !== null &&
					"content" in row &&
					Array.isArray(row.content)
						? row.content
						: [];
				return cells.map((cell) => tiptapText(cell, depth + 2)).join(" ");
			})
			.join("\n");
	}
	const parts = n.content
		.map((child) => tiptapText(child, depth + 1))
		.filter((s) => s.length > 0);
	if (
		n.type === "doc" ||
		n.type === "blockquote" ||
		n.type === "bulletList" ||
		n.type === "orderedList" ||
		n.type === "listItem" ||
		n.type === "callout" ||
		n.type === "details" ||
		n.type === "detailsContent" ||
		n.type === "taskList" ||
		n.type === "taskItem"
	) {
		return parts.join("\n");
	}
	return parts.join("");
}

export function extractText(root: unknown): string {
	if (typeof root !== "object" || root === null) return "";
	const n = root as { type?: unknown; content?: unknown[] };
	if (n.type === "doc" && Array.isArray(n.content)) {
		return n.content
			.map((child) => tiptapText(child, 1))
			.filter((s) => s.length > 0)
			.join("\n");
	}
	return tiptapText(root, 0);
}

export type InternalRef = { kind: "document" | "task"; id: string };

export type TiptapWalkNode = {
	type?: unknown;
	attrs?: Record<string, unknown>;
	content?: unknown[];
	text?: unknown;
	marks?: unknown;
};

export function walkTiptap(
	node: unknown,
	visit: (n: TiptapWalkNode) => void,
	depth = 0,
): void {
	if (depth > TIPTAP_WALK_MAX_DEPTH) return;
	if (typeof node !== "object" || node === null) return;
	const n = node as TiptapWalkNode;
	visit(n);
	if (Array.isArray(n.content)) {
		for (const child of n.content) walkTiptap(child, visit, depth + 1);
	}
}

export const UNIQUE_ID_NODE_TYPES = [
	"heading",
	"paragraph",
	"blockquote",
	"codeBlock",
	"embed",
	"listItem",
	"table",
	"horizontalRule",
	"callout",
	"mermaid",
	"math",
	"details",
	"detailsContent",
	"detailsSummary",
	"taskList",
	"taskItem",
] as const;

const UNIQUE_ID_NODE_TYPE_SET: ReadonlySet<string> = new Set(
	UNIQUE_ID_NODE_TYPES,
);

export function replaceTiptapNodeById(
	doc: TiptapDoc,
	blockId: string,
	node: TiptapWalkNode,
): TiptapDoc | null {
	const next = structuredClone(doc);
	let hit = false;
	walkTiptap(next, (n) => {
		if (hit) return;
		if (n.attrs?.id !== blockId) return;
		if (typeof n.type !== "string" || !UNIQUE_ID_NODE_TYPE_SET.has(n.type)) {
			return;
		}
		hit = true;
		const incoming = structuredClone(node);
		n.type = incoming.type;
		n.attrs = { ...(incoming.attrs ?? {}), id: blockId };
		if (incoming.content !== undefined) n.content = incoming.content;
		else delete n.content;
		if (incoming.text !== undefined) n.text = incoming.text;
		else delete n.text;
		if (incoming.marks !== undefined) n.marks = incoming.marks;
		else delete n.marks;
	});
	return hit ? next : null;
}

export function extractInternalRefs(root: unknown): InternalRef[] {
	const seen = new Set<string>();
	const out: InternalRef[] = [];
	const add = (kind: InternalRef["kind"], id: string): void => {
		if (!uuid.safeParse(id).success) return;
		const key = `${kind}:${id}`;
		if (seen.has(key)) return;
		seen.add(key);
		out.push({ kind, id });
	};
	walkTiptap(root, (n) => {
		if (n.type === "mention") {
			const entity = str(n.attrs?.entity);
			if (entity === "document" || entity === "task")
				add(entity, str(n.attrs?.id));
		}
		if (n.type === "embed") {
			const entity = str(n.attrs?.entity);
			if (entity === "document" || entity === "task")
				add(entity, str(n.attrs?.ref));
		}
	});
	return out;
}

const CHOSUNG = [
	"ㄱ",
	"ㄲ",
	"ㄴ",
	"ㄷ",
	"ㄸ",
	"ㄹ",
	"ㅁ",
	"ㅂ",
	"ㅃ",
	"ㅅ",
	"ㅆ",
	"ㅇ",
	"ㅈ",
	"ㅉ",
	"ㅊ",
	"ㅋ",
	"ㅌ",
	"ㅍ",
	"ㅎ",
] as const;

const HANGUL_FIRST = 0xac00;
const HANGUL_LAST = 0xd7a3;
const SYLLABLES_PER_CHOSUNG = 21 * 28;

export function toChosung(text: string): string {
	return Array.from(text)
		.map((ch) => {
			const cp = ch.codePointAt(0) ?? 0;
			if (cp < HANGUL_FIRST || cp > HANGUL_LAST) return ch;
			const idx = Math.floor((cp - HANGUL_FIRST) / SYLLABLES_PER_CHOSUNG);
			return CHOSUNG[idx] ?? ch;
		})
		.join("");
}
