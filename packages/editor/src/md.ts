import { emojiGlyph } from "./emoji-glyph.js";
import type { TiptapDoc } from "./json.js";
import { mdToTiptapJson } from "./markdown/parse.js";

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

function marksOf(n: unknown): Array<{ type: string; href?: string }> {
	if (!isRecord(n) || !Array.isArray(n.marks)) return [];
	const out: Array<{ type: string; href?: string }> = [];
	for (const m of n.marks) {
		if (!isRecord(m) || typeof m.type !== "string") continue;
		const href =
			isRecord(m.attrs) && typeof m.attrs.href === "string"
				? m.attrs.href
				: undefined;
		// WHY: #727 — only emitted marks may split text runs before escaping their boundaries.
		if (
			["code", "bold", "italic", "strike", "highlight"].includes(m.type) ||
			(m.type === "link" && href)
		)
			out.push({ type: m.type, href });
	}
	return out;
}

function escapeMdInline(text: string, opts: MdOpts): string {
	/* WHY: #727 — consume each slash run once, including runs not followed by `<`.
	 * Doubling only slashes before an angle preserves its escape without changing how
	 * existing backslashes protect other Markdown punctuation. Non-emitted marks have
	 * already been removed, so adjacent literal text is escaped as one emitted run. */
	const literal = opts.rawHtml
		? text
		: text.replace(/\\+|</g, (run, offset: number) =>
				run === "<"
					? "\\<"
					: text[offset + run.length] === "<"
						? run.repeat(2)
						: run,
			);
	const out = literal.replaceAll("![", "!\\[");
	return opts.escapeDollars ? out.replaceAll("$", "\\$") : out;
}

function escapeMdLineStarts(
	text: string,
	mathFirst: boolean,
	codeFirst: boolean,
): string {
	return text
		.split("\n")
		.map((line, index) => {
			/* WHY: #659 A — `$$` 줄은 재파싱에서 수식 펜스를 열어 뒤 문단까지 아톰으로 삼킨다.
			 * 선두 `$` 런을 통째로 죽인다 — 하나라도 살려 두면 같은 줄 뒤 `$` 와 짝을 이뤄
			 * 인라인 수식이 된다(rev-684 F1·N1). 홑 `$` 는 펜스가 아니라 그대로 둔다
			 * (micromark-extension-math math-flow.js `sizeOpen < 2 → nok`). */
			/* WHY: rev-691 F1 — 선두 런이 자기가 낸 울타리인지 문자열로 되추측하지 않는다.
			 * 직렬화기는 그 줄의 첫 인라인 노드가 수식인지 **안다**. 그 경우만 건너뛴다. */
			if (line.startsWith("$$") && !(mathFirst && index === 0)) {
				return line.replace(/^\$+/, (m) => "\\$".repeat(m.length));
			}
			if (
				line.startsWith("#") ||
				line.startsWith(">") ||
				line.startsWith("-") ||
				(line.startsWith("`") && !(codeFirst && index === 0))
			) {
				return `\\${line}`;
			}
			return line.replace(/^(\d+)\./, "$1\\.");
		})
		.join("\n");
}

/* WHY: #688 — latex 는 임의 문자열이다. `$` 가 섞이면 `$a$b$` 가 일찍 닫혀 뒤 문장이 수식으로
 * 빨려 들어간다. 블록(#656 F3)과 같이 내용의 최장 `$` 연속보다 한 칸 긴 울타리를 쓰고, 양끝이
 * `$` 면 공백 한 칸을 덧댄다(math-text 가 그 한 쌍을 벗긴다). 개행은 문단을 끊으므로 공백으로
 * 접는다 — 인라인 LaTeX 는 공백 연속을 구분하지 않는다. */
function mathInlineMd(raw: string, wide: boolean): string {
	const latex = raw.replace(/\s+/g, " ").trim();
	if (latex === "") return "";
	const fence = fenceFor(latex, "$", wide ? 2 : 1);
	const pad = latex.startsWith("$") || latex.endsWith("$") ? " " : "";
	return `${fence}${pad}${latex}${pad}${fence}`;
}

type MdRun = { md: string; marks: ReturnType<typeof marksOf> };

/* WHY: #656 F5 · #688 — 마크 문법은 노드 하나가 아니라 같은 마크가 이어지는 구간 전체를
 * 감싸야 한다. 조각마다 감싸면 `**bold $x$**` 가 `**bold ****$x$**`(텍스트 분해) 또는
 * `**bold **$x$`(인라인 수식 노드)로 나가고, 후자는 CommonMark 가 닫지도 못한다. */
/* WHY: rev-691 F6 — 이스케이프는 조각이 아니라 합친 텍스트에 걸어야 한다. `[text "a!", text "[b](x)"]`
 * 를 따로 이스케이프하면 경계의 `![` 를 놓쳐 이미지로 재파싱된다. */
function mergeTextNodes(nodes: unknown[]): unknown[] {
	const out: unknown[] = [];
	for (const n of nodes) {
		const prev = out.at(-1);
		if (
			isRecord(n) &&
			isRecord(prev) &&
			typeof n.text === "string" &&
			typeof prev.text === "string" &&
			JSON.stringify(marksOf(prev)) === JSON.stringify(marksOf(n))
		) {
			out[out.length - 1] = { ...prev, text: `${prev.text}${n.text}` };
			continue;
		}
		out.push(n);
	}
	return out;
}

function mdRuns(nodes: unknown[], opts: MdOpts): MdRun[] {
	const out: MdRun[] = [];
	for (const n of mergeTextNodes(nodes)) {
		const t = nodeType(n);
		const marks = marksOf(n);
		let md: string;
		if (t === "text") {
			const text = isRecord(n) && typeof n.text === "string" ? n.text : "";
			/* WHY: rev-691 — 코드 마크 안은 이스케이프하지 않는다. 백슬래시가 코드 스팬 안에서는
			 * 리터럴이라, 여기서 `$` 를 죽이면 리포 문서 32건의 코드 조각이 그대로 오염된다. */
			md = marks.some((m) => m.type === "code")
				? text
				: escapeMdInline(text, opts);
		} else if (t === "mathInline") {
			md = mathInlineMd(strAttr(n, "latex"), opts.wideMath === true);
			/* WHY: rev-691 F4 — 마크다운은 `$a$$b$` 의 가운데 `$$` 를 한 런으로 읽어 두 수식을
			 * 하나로 합친다. 사이에 아무것도 없을 때만 빈 주석을 끼운다 — 파서가 인라인 html 을
			 * 버리므로 왕복은 그대로고, 다른 렌더러에서도 보이지 않는다. */
			if (md !== "" && (out.at(-1)?.md ?? "").endsWith("$"))
				md = `<!---->${md}`;
		} else if (t === "hardBreak") {
			/* WHY: #500 — CommonMark 의 줄바꿈 하드브레이크(`"  \n"`/`"\\\n"`)는 GFM 표 셀의
			 * "행은 한 줄" 규칙과 함께 쓸 수 없다. 문단·셀 모두 `<br>` 하나로 통일해 컨텍스트별
			 * 분기 없이 같은 불변식(출력은 항상 한 줄 텍스트)을 유지한다. */
			md = "<br>";
		} else if (t === "mention") {
			const label = strAttr(n, "label");
			md = label ? `@${label}` : "";
		} else if (t === "emoji") {
			md = emojiGlyph(n);
		} else {
			out.push(...mdRuns(nodeContent(n) ?? [], opts));
			continue;
		}
		const prev = out.at(-1);
		if (prev && JSON.stringify(prev.marks) === JSON.stringify(marks)) {
			prev.md += md;
			continue;
		}
		out.push({ md, marks });
	}
	return out;
}

function fenceFor(text: string, marker: "`" | "$", minimum: number): string {
	let length = minimum;
	for (const match of text.matchAll(marker === "`" ? /`+/g : /\$+/g)) {
		length = Math.max(length, match[0].length + 1);
	}
	return marker.repeat(length);
}

function literalText(nodes: unknown[] | undefined): string {
	return (nodes ?? [])
		.map((n) => (isRecord(n) && typeof n.text === "string" ? n.text : ""))
		.join("");
}

function fencedCode(text: string, language: string): string {
	const fence = fenceFor(text, "`", 3);
	return `${fence}${language}\n${text}\n${fence}`;
}

function wrapMdMarks({ md, marks }: MdRun): string {
	let text = md;
	if (marks.some((m) => m.type === "code")) {
		const fence = fenceFor(text, "`", 1);
		const pad =
			text.startsWith("`") ||
			text.endsWith("`") ||
			(/^ .* $/.test(text) && /[^ ]/.test(text))
				? " "
				: "";
		text = `${fence}${pad}${text}${pad}${fence}`;
	}
	if (marks.some((m) => m.type === "bold")) text = `**${text}**`;
	if (marks.some((m) => m.type === "italic")) text = `*${text}*`;
	if (marks.some((m) => m.type === "strike")) text = `~~${text}~~`;
	if (marks.some((m) => m.type === "highlight")) text = `==${text}==`;
	const link = marks.find((m) => m.type === "link" && m.href);
	return link?.href ? `[${text}](${link.href})` : text;
}

/** WHY: rev-691·#727 — 수식 자기 검증 재시도와 raw HTML summary 의 이스케이프 문맥. */
type MdOpts = {
	escapeDollars?: boolean;
	wideMath?: boolean;
	rawHtml?: boolean;
};

function inlineMd(nodes: unknown[] | undefined, opts: MdOpts = {}): string {
	if (!nodes) return "";
	return mdRuns(nodes, opts).map(wrapMdMarks).join("");
}

/* WHY: rev-691 — 「텍스트인가 수식인가」의 정본은 파서다. 직렬화가 낸 마크다운을 그대로 되읽어
 * 수식/텍스트 배치가 원본과 같은지 본다. 다르면(텍스트 `$` 가 수식이 됐거나 수식이 텍스트로
 * 강등됐거나) 텍스트의 `$` 를 이스케이프하고 울타리를 넓혀 한 번만 더 낸다. 문자열 추측 규칙을
 * 늘리는 대신 파서를 오라클로 쓴다 — `$` 가 없는 줄은 검증을 건너뛴다. */
function pushMathShape(out: string[], n: unknown): void {
	const t = nodeType(n);
	if (t === "mathInline") {
		out.push(`\u0001${strAttr(n, "latex")}\u0002`);
		return;
	}
	if (t === "text") {
		out.push(isRecord(n) && typeof n.text === "string" ? n.text : "");
		return;
	}
	if (t === "hardBreak") {
		out.push("\n");
		return;
	}
	if (t === "mention") {
		const label = strAttr(n, "label");
		out.push(label ? `@${label}` : "");
		return;
	}
	if (t === "emoji") {
		out.push(emojiGlyph(n));
		return;
	}
	for (const child of nodeContent(n) ?? []) pushMathShape(out, child);
}

function mathShape(nodes: unknown[] | undefined): string {
	const out: string[] = [];
	for (const n of nodes ?? []) pushMathShape(out, n);
	return out.join("");
}

function shapeOfMd(md: string): string {
	return mathShape(mdToTiptapJson(`${md}\n`).content);
}

function selfCheckedMd(
	content: unknown[] | undefined,
	wrap: (inline: string, opts: MdOpts) => string,
): string {
	const first = wrap(inlineMd(content), {});
	// WHY: #727 — angles are escaped unconditionally, including tables; the retry only repairs math.
	if (!first.includes("$")) return first;
	const want = mathShape(content);
	if (want === shapeOfMd(first)) return first;
	const opts: MdOpts = { escapeDollars: true, wideMath: true };
	const second = wrap(inlineMd(content, opts), opts);
	/* WHY: rev-691 — 2패스는 더 나아질 때만 받는다. 코드 스팬 안에서는 백슬래시가 리터럴이라
	 * `\$` 가 본문을 더럽힌다(리포 문서 5건 실측). 파서가 아니라고 하면 1패스를 그대로 낸다. */
	return shapeOfMd(second) === want ? second : first;
}

function cellText(cell: unknown): string {
	return (nodeContent(cell) ?? [])
		.map((p) => inlineMd(nodeContent(p)))
		.join(" ")
		.replace(/\|/g, "\\|");
}

function gfmTable(n: unknown): string {
	const rows = (nodeContent(n) ?? []).filter((r) => nodeType(r) === "tableRow");
	const cellsOf = (row: unknown) => nodeContent(row) ?? [];
	const first = rows[0];
	if (!first) return "";
	const header = cellsOf(first);
	const body = rows.slice(1);
	const line = (cells: unknown[]) => `| ${cells.map(cellText).join(" | ")} |`;
	const sep = `| ${header.map(() => "---").join(" | ")} |`;
	return [line(header), sep, ...body.map((r) => line(cellsOf(r)))].join("\n");
}

function listMd(n: unknown, ordered: boolean, task = false): string {
	return (nodeContent(n) ?? [])
		.map((item, i) => {
			const marker = task
				? nodeAttr(item, "checked") === true
					? "- [x] "
					: "- [ ] "
				: ordered
					? `${i + 1}. `
					: "- ";
			const inner = blocksMd(nodeContent(item));
			const [head, ...rest] = inner.split("\n");
			const first = `${marker}${head ?? ""}`;
			if (rest.length === 0) return first;
			/* WHY: #659 B — 이어지는 블록은 마커 폭만큼 들여써야 같은 항목에 붙는다(`1. ` 은 3칸).
			 * 태스크 체크박스는 GFM 이 첫 문단 내용으로 먹으므로 폭은 `- ` 와 같은 2칸이다. */
			const pad = " ".repeat(task ? 2 : marker.length);
			return [first, ...rest.map((l) => (l ? `${pad}${l}` : l))].join("\n");
		})
		.join("\n");
}

function blockMd(n: unknown): string {
	const t = nodeType(n);
	const content = nodeContent(n);
	switch (t) {
		case "paragraph": {
			const mathFirst = nodeType((content ?? [])[0]) === "mathInline";
			return selfCheckedMd(content, (inline) =>
				escapeMdLineStarts(
					inline,
					mathFirst,
					marksOf((content ?? [])[0]).some((m) => m.type === "code"),
				),
			);
		}
		case "heading": {
			const level = nodeAttr(n, "level");
			const nLevel =
				typeof level === "number" ? Math.min(6, Math.max(1, level)) : 1;
			return selfCheckedMd(
				content,
				(inline) => `${"#".repeat(nLevel)} ${inline}`,
			);
		}
		case "blockquote":
			return blocksMd(content)
				.split("\n")
				.map((l) => `> ${l}`)
				.join("\n");
		case "callout": {
			const kind = strAttr(n, "kind").toUpperCase() || "NOTE";
			const inner = blocksMd(content);
			const lines = inner.length > 0 ? inner.split("\n") : [""];
			return [`> [!${kind}]`, ...lines.map((l) => `> ${l}`)].join("\n");
		}
		case "details": {
			const kids = content ?? [];
			const summaryNode = kids.find((c) => nodeType(c) === "detailsSummary");
			const bodyNode = kids.find((c) => nodeType(c) === "detailsContent");
			// WHY: #727 — summary 는 파서가 raw HTML 텍스트로 읽으므로 Markdown의 백슬래시·`<` 이스케이프를 하지 않는다.
			const summary = inlineMd(nodeContent(summaryNode), {
				rawHtml: true,
			});
			const body = blocksMd(nodeContent(bodyNode));
			return `<details><summary>${summary}</summary>\n\n${body}\n\n</details>`;
		}
		case "math": {
			const latex = strAttr(n, "latex");
			/* WHY: #656 F3 — textarea 는 임의 다줄 텍스트를 커밋한다. latex 안에 `$$` 줄이 있으면
			 * 재파싱에서 그 줄이 펜스를 닫고 뒤 문단이 다음 수식으로 빨려 들어간다. mdast-util-math
			 * 처럼 내용의 최장 `$` 연속보다 한 칸 긴 펜스를 쓴다. */
			const fence = fenceFor(latex, "$", 2);
			return `${fence}\n${latex}\n${fence}`;
		}
		case "mermaid": {
			const source = strAttr(n, "source") || literalText(content);
			return fencedCode(source, "mermaid");
		}
		case "attachment": {
			const id = strAttr(n, "id");
			const name = strAttr(n, "name");
			const image = nodeAttr(n, "image") === true;
			return image
				? `![${name}](attachment:${id})`
				: `[${name}](attachment:${id})`;
		}
		case "taskList":
			return listMd(n, false, true);
		case "taskItem":
			return blocksMd(content);
		case "codeBlock": {
			const lang = strAttr(n, "language");
			return fencedCode(literalText(content), lang);
		}
		case "bulletList":
			return listMd(n, false);
		case "orderedList":
			return listMd(n, true);
		case "listItem":
			return blocksMd(content);
		case "horizontalRule":
			return "---";
		case "table":
			return gfmTable(n);
		case "embed": {
			const entity = strAttr(n, "entity") || "document";
			const ref = strAttr(n, "ref");
			if (entity === "url") return ref;
			return `[[${entity === "document" ? "doc" : entity}:${ref}]]`;
		}
		default:
			return content ? blocksMd(content) : inlineMd(content);
	}
}

function blocksMd(nodes: unknown[] | undefined): string {
	if (!nodes) return "";
	return nodes
		.map(blockMd)
		.filter((s) => s.length > 0)
		.join("\n\n");
}

export function tiptapDocToMd(doc: TiptapDoc): string {
	const md = blocksMd(doc.content);
	return md.length > 0 ? `${md}\n` : "";
}
