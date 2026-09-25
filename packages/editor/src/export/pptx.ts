// packages/editor/src/export/pptx.ts
import PptxGenJS from "@jsamuel1/pptxgenjs";
import { emojiGlyph } from "../emoji-glyph.js";
import type { TiptapDoc } from "../json.js";
import { ExportLimitError } from "./limits.js";

const BODY = "Noto Sans KR";
const MONO = "Noto Sans Mono CJK KR";
const X = 0.5;
const W = 9.0;
const SLIDE_H = 5.625;
const TOP = 0.4;
const BOTTOM = 0.4;
const MIN_H = 0.34;

type Pres = InstanceType<typeof PptxGenJS>;
type Slide = ReturnType<Pres["addSlide"]>;
type Cursor = { pres: Pres; slide: Slide; y: number; headings: number };

function isRecord(v: unknown): v is Record<string, unknown> {
	return typeof v === "object" && v !== null;
}

function nodeType(n: unknown): string {
	return isRecord(n) && typeof n.type === "string" ? n.type : "";
}

function nodeContent(n: unknown): unknown[] | undefined {
	return isRecord(n) && Array.isArray(n.content) ? n.content : undefined;
}

function strAttr(n: unknown, key: string): string {
	if (!isRecord(n) || !isRecord(n.attrs)) return "";
	const v = n.attrs[key];
	return typeof v === "string" ? v : "";
}

function inlineText(nodes: unknown[] | undefined): string {
	if (!nodes) return "";
	let out = "";
	for (const n of nodes) {
		const t = nodeType(n);
		if (t === "text") {
			out += isRecord(n) && typeof n.text === "string" ? n.text : "";
			continue;
		}
		if (t === "mention") {
			out += `@${strAttr(n, "label")}`;
			continue;
		}
		// WHY: #688 — 블록 수식과 같이 LaTeX 원문을 남긴다. 아톰이라 자식 순회로는 아무것도 없다.
		if (t === "mathInline") {
			out += strAttr(n, "latex");
			continue;
		}
		if (t === "hardBreak") {
			out += "\n";
			continue;
		}
		if (t === "emoji") {
			out += emojiGlyph(n);
			continue;
		}
		if (t === "image" || t === "attachment") {
			out +=
				strAttr(n, "name") ||
				strAttr(n, "title") ||
				strAttr(n, "fileName") ||
				strAttr(n, "src");
			continue;
		}
		out += inlineText(nodeContent(n));
	}
	return out;
}

function blocksText(nodes: unknown[] | undefined): string {
	if (!nodes) return "";
	return nodes
		.map((n) => inlineText(nodeContent(n)) || inlineText([n]))
		.filter((s) => s.length > 0)
		.join("\n");
}

function toBuffer(value: string | ArrayBuffer | Blob | Uint8Array): Buffer {
	if (Buffer.isBuffer(value)) return value;
	if (value instanceof Uint8Array) return Buffer.from(value);
	throw new Error("pptxgenjs write");
}

function remaining(cur: Cursor): number {
	return SLIDE_H - BOTTOM - cur.y;
}

function addSlide(cur: Cursor): void {
	cur.slide = cur.pres.addSlide();
	cur.y = TOP;
	cur.headings = 0;
}

function estimateH(
	text: string,
	fontSize: number,
	w: number,
	maxH: number,
): number {
	const cpl = Math.max(8, Math.floor((w * 72) / fontSize));
	let lines = 0;
	for (const line of text.split("\n")) {
		lines += Math.max(1, Math.ceil([...line].length / cpl));
	}
	return Math.min(maxH, Math.max(MIN_H, lines * (fontSize / 72) * 1.25));
}

function addBody(
	cur: Cursor,
	text: string,
	opts: {
		fontFace?: string;
		fontSize?: number;
		h?: number;
		x?: number;
		w?: number;
		bold?: boolean;
		bullet?: true | { type: "number" };
	} = {},
): void {
	if (text.length === 0) return;
	const w = opts.w ?? W;
	const fontSize = opts.fontSize ?? 14;
	const compact = opts.h;
	if (remaining(cur) < (compact ?? MIN_H)) addSlide(cur);
	const rest = remaining(cur);
	const h = compact ?? rest;
	cur.slide.addText(text, {
		x: opts.x ?? X,
		y: cur.y,
		w,
		h,
		fontFace: opts.fontFace ?? BODY,
		fontSize,
		valign: "top",
		fit: "shrink",
		...(opts.bold ? { bold: true } : {}),
		...(opts.bullet === undefined ? {} : { bullet: opts.bullet }),
	});
	cur.y += (compact ?? estimateH(text, fontSize, w, rest)) + 0.04;
}

function addBlock(cur: Cursor, n: unknown): void {
	const t = nodeType(n);
	const content = nodeContent(n);
	if (t === "heading") {
		const first = cur.headings === 0;
		cur.headings += 1;
		addBody(cur, inlineText(content), {
			fontSize: first ? 28 : 16,
			h: first ? 0.55 : 0.38,
			bold: true,
		});
		return;
	}
	if (t === "paragraph") {
		addBody(cur, inlineText(content));
		return;
	}
	if (t === "bulletList" || t === "orderedList") {
		const numbered = t === "orderedList";
		for (const item of content ?? []) {
			addBody(cur, blocksText(nodeContent(item)), {
				bullet: numbered ? { type: "number" } : true,
			});
		}
		return;
	}
	if (t === "table") {
		const rows = (content ?? [])
			.filter((r) => nodeType(r) === "tableRow")
			.map((row) =>
				(nodeContent(row) ?? []).map((cell) => ({
					text: blocksText(nodeContent(cell)),
					options: { fontFace: BODY, fontSize: 12 },
				})),
			);
		if (rows.length === 0) return;
		const h = Math.max(0.4, rows.length * 0.32);
		cur.slide.addTable(rows, { x: X, y: cur.y, w: W });
		cur.y += h + 0.08;
		return;
	}
	if (t === "codeBlock") {
		addBody(cur, inlineText(content), {
			fontFace: MONO,
			fontSize: 12,
		});
		return;
	}
	// WHY: #656 F2 — 아톰이라 말미의 자식 순회가 아무것도 못 내보낸다. LaTeX 원문을 mono 로 남긴다.
	if (t === "math") {
		addBody(cur, strAttr(n, "latex"), { fontFace: MONO, fontSize: 12 });
		return;
	}
	// WHY: #659 C — mermaid 도 아톰이라 같은 자리가 비었다. 다이어그램 원문을 mono 로 남긴다.
	if (t === "mermaid") {
		addBody(cur, strAttr(n, "source"), { fontFace: MONO, fontSize: 12 });
		return;
	}
	if (t === "blockquote") {
		addBody(cur, blocksText(content), { x: X + 0.3, w: W - 0.3 });
		return;
	}
	if (t === "image" || t === "attachment") {
		addBody(cur, inlineText([n]));
		return;
	}
	if (t === "embed") {
		addBody(cur, strAttr(n, "ref") || strAttr(n, "url"));
		return;
	}
	if (t === "horizontalRule") return;
	for (const child of content ?? []) addBlock(cur, child);
}

export async function tiptapDocToPptx(
	doc: TiptapDoc,
	opts?: { title?: string },
): Promise<Buffer> {
	const pres = new PptxGenJS();
	if (opts?.title) pres.title = opts.title;
	const cur: Cursor = { pres, slide: pres.addSlide(), y: TOP, headings: 0 };
	for (const n of doc.content ?? []) {
		if (nodeType(n) === "horizontalRule") {
			addSlide(cur);
			continue;
		}
		addBlock(cur, n);
	}
	const buffer = toBuffer(await pres.write({ outputType: "nodebuffer" }));
	if (buffer.byteLength > 20_000_000) {
		throw new ExportLimitError("maxOutputBytes");
	}
	return buffer;
}
