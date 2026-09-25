import { emojiGlyph } from "./emoji-glyph.js";
import type { TiptapDoc } from "./json.js";
import { sanitizeRenderedHtml } from "./sanitize.js";

function isRecord(v: unknown): v is Record<string, unknown> {
	return typeof v === "object" && v !== null;
}

function nodeType(n: unknown): string {
	return isRecord(n) && typeof n.type === "string" ? n.type : "";
}

function nodeContent(n: unknown): unknown[] | undefined {
	return isRecord(n) && Array.isArray(n.content) ? n.content : undefined;
}

function nodeAttr(n: unknown, key: string): unknown {
	if (!isRecord(n) || !isRecord(n.attrs)) return undefined;
	return n.attrs[key];
}

function strAttr(n: unknown, key: string): string {
	const v = nodeAttr(n, key);
	return typeof v === "string" ? v : "";
}

function esc(s: string): string {
	return s
		.replaceAll("&", "&amp;")
		.replaceAll("<", "&lt;")
		.replaceAll(">", "&gt;")
		.replaceAll('"', "&quot;");
}

function marksOf(n: unknown): Array<{ type: string; href?: string }> {
	if (!isRecord(n) || !Array.isArray(n.marks)) return [];
	const out: Array<{ type: string; href?: string }> = [];
	for (const m of n.marks) {
		if (!isRecord(m) || typeof m.type !== "string") continue;
		const href =
			isRecord(m.attrs) && typeof m.attrs.href === "string"
				? m.attrs.href
				: undefined;
		out.push({ type: m.type, href });
	}
	return out;
}

function wrapMarks(
	html: string,
	marks: Array<{ type: string; href?: string }>,
): string {
	let out = html;
	for (const m of marks) {
		if (m.type === "bold") out = `<strong>${out}</strong>`;
		else if (m.type === "italic") out = `<em>${out}</em>`;
		else if (m.type === "strike") out = `<s>${out}</s>`;
		else if (m.type === "code") out = `<code>${out}</code>`;
		else if (m.type === "link" && m.href) {
			out = `<a href="${esc(m.href)}">${out}</a>`;
		}
	}
	return out;
}

function inlineHtml(nodes: unknown[] | undefined): string {
	if (!nodes) return "";
	return nodes
		.map((n) => {
			const t = nodeType(n);
			if (t === "hardBreak") return "<br>";
			if (t === "mention") {
				const label = strAttr(n, "label");
				return `@${esc(label)}`;
			}
			if (t === "text") {
				const text = isRecord(n) && typeof n.text === "string" ? n.text : "";
				return wrapMarks(esc(text), marksOf(n));
			}
			/* WHY: #688 — mathInline 도 아톰이라 default 가 자식 순회로 빈 문자열을 낸다. 블록
			 * 수식과 같이 LaTeX 원문을 남긴다 — MathML 은 서버에서 그리지 않는다. */
			if (t === "mathInline") {
				const latex = esc(strAttr(n, "latex"));
				return wrapMarks(
					`<span data-math-inline="">${latex}</span>`,
					marksOf(n),
				);
			}
			if (t === "emoji") return esc(emojiGlyph(n));
			return inlineHtml(nodeContent(n));
		})
		.join("");
}

function listHtml(n: unknown, tag: "ul" | "ol"): string {
	const items = (nodeContent(n) ?? [])
		.map((item) => `<li>${blocksHtml(nodeContent(item))}</li>`)
		.join("");
	return `<${tag}>${items}</${tag}>`;
}

function tableHtml(n: unknown): string {
	const rows = (nodeContent(n) ?? [])
		.filter((r) => nodeType(r) === "tableRow")
		.map((row) => {
			const cells = (nodeContent(row) ?? [])
				.map((cell) => {
					const tag = nodeType(cell) === "tableHeader" ? "th" : "td";
					return `<${tag}>${blocksHtml(nodeContent(cell))}</${tag}>`;
				})
				.join("");
			return `<tr>${cells}</tr>`;
		})
		.join("");
	return `<table>${rows}</table>`;
}

function blockHtml(n: unknown): string {
	const t = nodeType(n);
	const content = nodeContent(n);
	switch (t) {
		case "attachment":
			return `<p>${esc(strAttr(n, "name"))}</p>`;
		case "paragraph":
			return `<p>${inlineHtml(content)}</p>`;
		case "heading": {
			const level = nodeAttr(n, "level");
			const nLevel =
				typeof level === "number" ? Math.min(6, Math.max(1, level)) : 1;
			return `<h${nLevel}>${inlineHtml(content)}</h${nLevel}>`;
		}
		case "blockquote":
			return `<blockquote>${blocksHtml(content)}</blockquote>`;
		case "codeBlock":
			return `<pre><code>${inlineHtml(content)}</code></pre>`;
		case "bulletList":
			return listHtml(n, "ul");
		case "orderedList":
			return listHtml(n, "ol");
		case "listItem":
			return blocksHtml(content);
		case "horizontalRule":
			return "<hr>";
		/* WHY: #656 F1 — math 는 아톰이라 default 가 빈 문자열을 낸다. 공개 공유(format=html·
		 * fragment)와 이메일이 이 함수를 거쳐 수식이 흔적 없이 사라졌다. sanitize 는 pre·data-math
		 * 를 이미 허용한다 — MathML 은 서버에서 그리지 않고 LaTeX 원문을 그대로 남긴다. */
		case "math":
			return `<pre data-math="">${esc(strAttr(n, "latex"))}</pre>`;
		/* WHY: #659 C — mermaid 도 아톰이라 같은 구멍이 남아 있었다. sanitize 는 pre·data-mermaid
		 * 를 이미 허용하고, Mermaid.parseHTML 이 textContent 를 source 로 되읽는다. */
		case "mermaid":
			return `<pre data-mermaid="">${esc(strAttr(n, "source"))}</pre>`;
		case "table":
			return tableHtml(n);
		case "embed": {
			const ref = strAttr(n, "ref");
			return `<div data-embed="">${esc(ref)}</div>`;
		}
		default:
			return content ? blocksHtml(content) : inlineHtml(content);
	}
}

function blocksHtml(nodes: unknown[] | undefined): string {
	if (!nodes) return "";
	return nodes.map(blockHtml).join("");
}

export function tiptapDocToHtml(doc: TiptapDoc): string {
	return blocksHtml(doc.content);
}

export function tiptapDocToSafeHtml(doc: TiptapDoc): string {
	return sanitizeRenderedHtml(tiptapDocToHtml(doc));
}
