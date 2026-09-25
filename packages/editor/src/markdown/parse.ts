import remarkGfm from "remark-gfm";
import remarkMath from "remark-math";
import remarkParse from "remark-parse";
import { unified } from "unified";
import { emptyDocumentJson, type TiptapDoc } from "../json.js";
import { isCalloutKind } from "../nodes/callout.js";

/* WHY: #688 — remark-math 는 켠 채로 둔다. 어떤 `$` 가 구분자이고 어떤 것이 `\$` 이스케이프인지
 * 아는 곳은 micromark 뿐이다(mdast 텍스트에는 그 정보가 없다). 대신 code-text 규칙이라 통화
 * 표기까지 물으므로, 문 것을 Pandoc 규칙으로 한 번 더 거르고 거른 자리만 다시 훑는다. */
const parser = unified().use(remarkParse).use(remarkGfm).use(remarkMath);

type Root = ReturnType<typeof parser.parse>;
type RootNode = Root["children"][number];
type Blockquote = Extract<RootNode, { type: "blockquote" }>;
type Paragraph = Extract<RootNode, { type: "paragraph" }>;
type Phrasing = Paragraph["children"][number];
type ListNode = Extract<RootNode, { type: "list" }>;
type ListItemNode = ListNode["children"][number];
type TableNode = Extract<RootNode, { type: "table" }>;

type Mark = { type: string; attrs?: Record<string, string> };
type TipNode = {
	type: string;
	attrs?: Record<string, unknown>;
	content?: TipNode[];
	text?: string;
	marks?: Mark[];
};

const TOKEN_SRC = String.raw`\[\[(doc|task|project):([^|\]]+)(?:\|([^\]]+))?\]\]|@\[(user|group):([^|\]]+)\|([^\]]+)\]|==`;
const TOKEN = new RegExp(TOKEN_SRC, "g");

/* WHY: #688 — Pandoc·GitHub 규칙: 여는 `$` 뒤와 닫는 `$` 앞이 공백이 아니어야 하고 닫는 `$`
 * 뒤에 숫자가 오면 수식이 아니다. micromark 가 잘못 짝지어 되돌린 자리에만 쓴다 — 보통 텍스트에
 * 쓰면 `\$x\$` 처럼 이스케이프한 달러까지 수식이 된다. */
const TOKEN_MATH = new RegExp(
	String.raw`${TOKEN_SRC}|\$([^\s{$\n](?:[^$\n]*[^\s$\n])?)\$(?!\d)`,
	"g",
);
const REF_FULL = /^\[\[(doc|task|project):([^|\]]+)(?:\|([^\]]+))?\]\]$/;
const ATTACHMENT_URL = /^attachment:(.+)$/;
const CALLOUT_MARKER = /^\[!(NOTE|TIP|WARNING|CAUTION)\]\n?/;
const DETAILS_OPEN = /^<details>\s*<summary>([\s\S]*?)<\/summary>\s*$/;

const REF_ENTITY: Record<"doc" | "task" | "project", string> = {
	doc: "document",
	task: "task",
	project: "project",
};

/* WHY: rev-691 F7·F9 — `$` 판정에는 원문 조각이 필요하다. micromark 의 `value` 는 대칭 공백
 * 한 쌍을 벗기고(`$ x $` → `x`) `\$` 를 이미 리터럴 `$` 로 풀어 놓아, 그것만 보면 Pandoc 규칙을
 * 세울 수 없다. 파싱은 동기 1회라 진입점에서 원문을 잡아 둔다 — #684 가 걷어낸 src 인자
 * 스레딩을 되살리지 않기 위해서다. */
let source = "";

export function mdToTiptapJson(md: string): TiptapDoc {
	source = md;
	const tree = parser.parse(md);
	const content = blocksFromNodes(tree.children);
	return content.length > 0 ? { type: "doc", content } : emptyDocumentJson();
}

function blocksFromNodes(nodes: RootNode[]): TipNode[] {
	const out: TipNode[] = [];
	let i = 0;
	while (i < nodes.length) {
		const node = nodes[i];
		if (!node) {
			i++;
			continue;
		}
		if (node.type === "html") {
			const summary = detailsSummary(node.value);
			if (summary !== undefined) {
				/* WHY: rev-684 F2 — 여는·닫는 html 사이 형제 노드는 이미 컨테이너 접두가 벗겨진 채
				 * 파싱돼 있다. 원문을 절대 오프셋으로 다시 잘라 재파싱하면 인용 안에서는 `> ` 가
				 * 딸려 들어와 왕복마다 blockquote 가 한 겹씩 쌓인다. */
				let j = i + 1;
				while (j < nodes.length && !isDetailsClose(nodes[j])) j++;
				const inner = blocksFromNodes(nodes.slice(i + 1, j));
				out.push({
					type: "details",
					content: [
						{
							type: "detailsSummary",
							content:
								summary.length > 0 ? [{ type: "text", text: summary }] : [],
						},
						{
							type: "detailsContent",
							content: inner.length > 0 ? inner : [{ type: "paragraph" }],
						},
					],
				});
				i = j + 1;
				continue;
			}
		}
		if (node.type === "list") {
			out.push(listToNode(node));
			i++;
			continue;
		}
		const blk = nodeToBlock(node);
		if (blk) out.push(blk);
		i++;
	}
	return out;
}

function isDetailsClose(n: RootNode | undefined): boolean {
	return (
		n !== undefined && n.type === "html" && n.value.trim() === "</details>"
	);
}

function detailsSummary(value: string): string | undefined {
	const m = DETAILS_OPEN.exec(value);
	return m ? (m[1] ?? "") : undefined;
}

function nodeToBlock(node: RootNode): TipNode | undefined {
	switch (node.type) {
		case "paragraph":
			return paragraphToBlock(node);
		case "heading":
			return {
				type: "heading",
				attrs: { level: Math.min(6, Math.max(1, node.depth)) },
				content: mapPhrasing(node.children, []),
			};
		case "blockquote":
			return blockquoteToBlock(node);
		case "code":
			if ((node.lang ?? "").toLowerCase() === "mermaid") {
				return {
					type: "mermaid",
					attrs: { source: node.value },
				};
			}
			return {
				type: "codeBlock",
				attrs: { language: node.lang ?? "" },
				content:
					node.value.length > 0 ? [{ type: "text", text: node.value }] : [],
			};
		case "math":
			return { type: "math", attrs: { latex: node.value } };
		case "table":
			return tableToBlock(node);
		case "thematicBreak":
			return { type: "horizontalRule" };
		default:
			return undefined;
	}
}

function paragraphToBlock(p: Paragraph): TipNode {
	const children = p.children;
	const only = children.length === 1 ? children[0] : undefined;
	if (only) {
		if (only.type === "text") {
			const m = REF_FULL.exec(only.value.trim());
			if (m?.[1] && m[2] && !(m[1] === "doc" && m[3])) {
				const kind = m[1] as "doc" | "task" | "project";
				return {
					type: "embed",
					attrs: { entity: REF_ENTITY[kind], ref: m[2] },
				};
			}
		}
		if (only.type === "link" || only.type === "image") {
			const att = ATTACHMENT_URL.exec(only.url);
			if (att?.[1]) {
				const name =
					only.type === "image" ? (only.alt ?? "") : plainText(only.children);
				return {
					type: "attachment",
					attrs: {
						id: att[1],
						name,
						image: only.type === "image",
					},
				};
			}
		}
	}
	return { type: "paragraph", content: mapPhrasing(children, []) };
}

/* WHY: #659 B — 자식에서 paragraph 만 남기던 필터는 인용·콜아웃 안의 수식·코드블록·표를
 * 통째로 버렸다. 블록 변환은 blocksFromNodes 하나가 맡고, 여기는 콜아웃 마커만 벗긴다. */
function blockquoteToBlock(bq: Blockquote): TipNode {
	const first = bq.children[0];
	const firstNode = first?.type === "paragraph" ? first.children[0] : undefined;
	if (first?.type === "paragraph" && firstNode?.type === "text") {
		const m = CALLOUT_MARKER.exec(firstNode.value);
		if (m) {
			const remainder = firstNode.value.slice(m[0].length);
			const children: Phrasing[] =
				remainder.length > 0
					? [{ ...firstNode, value: remainder }, ...first.children.slice(1)]
					: first.children.slice(1);
			const kindRaw = m[1]?.toLowerCase() ?? "note";
			return {
				type: "callout",
				attrs: { kind: isCalloutKind(kindRaw) ? kindRaw : "note" },
				content: blocksFromNodes([
					{ ...first, children },
					...bq.children.slice(1),
				]),
			};
		}
	}
	return { type: "blockquote", content: blocksFromNodes(bq.children) };
}

function listItemChecked(li: ListItemNode): boolean | null {
	if (!("checked" in li)) return null;
	const value = li.checked;
	return typeof value === "boolean" ? value : null;
}

function listItemContent(li: ListItemNode): TipNode[] {
	const content = blocksFromNodes(li.children);
	return content.length > 0 ? content : [{ type: "paragraph" }];
}

function listToNode(list: ListNode): TipNode {
	const checks = list.children.map(listItemChecked);
	if (checks.some((c) => c !== null)) {
		return {
			type: "taskList",
			content: list.children.map((li, i) => ({
				type: "taskItem",
				attrs: { checked: checks[i] === true },
				content: listItemContent(li),
			})),
		};
	}
	return {
		type: list.ordered === true ? "orderedList" : "bulletList",
		content: list.children.map((li) => ({
			type: "listItem",
			content: listItemContent(li),
		})),
	};
}

function tableToBlock(t: TableNode): TipNode {
	return {
		type: "table",
		content: t.children.map((row, rowIndex) => ({
			type: "tableRow",
			content: row.children.map((cell) => ({
				type: rowIndex === 0 ? "tableHeader" : "tableCell",
				content: [
					{
						type: "paragraph",
						content: mapPhrasing(cell.children, []),
					},
				],
			})),
		})),
	};
}

function plainText(nodes: readonly Phrasing[]): string {
	return nodes.map(plainTextOf).join("");
}

function plainTextOf(n: Phrasing): string {
	switch (n.type) {
		case "text":
		case "inlineCode":
			return n.value;
		case "strong":
		case "emphasis":
		case "delete":
			return plainText(n.children);
		default:
			return "";
	}
}

function mapPhrasing(nodes: readonly Phrasing[], marks: Mark[]): TipNode[] {
	const out: TipNode[] = [];
	let underline = false;
	for (let i = 0; i < nodes.length; i++) {
		const n = nodes[i];
		if (n === undefined) continue;
		if (n.type === "html") {
			const v = n.value.trim();
			if (v === "<u>") underline = true;
			else if (v === "</u>") underline = false;
			// WHY: #500 — md.ts 가 hardBreak 를 항상 `<br>` 로 내므로 파서도 같은 태그를 되읽는다.
			else if (/^<br\s*\/?>$/i.test(v)) out.push({ type: "hardBreak" });
			continue;
		}
		const next = underline ? [...marks, { type: "underline" as const }] : marks;
		switch (n.type) {
			case "text":
				out.push(...tokenizeText(n.value, next));
				break;
			case "inlineCode":
				out.push(makeText(n.value, [...next, { type: "code" }]));
				break;
			case "strong":
				out.push(...mapPhrasing(n.children, [...next, { type: "bold" }]));
				break;
			case "emphasis":
				out.push(...mapPhrasing(n.children, [...next, { type: "italic" }]));
				break;
			case "delete":
				out.push(...mapPhrasing(n.children, [...next, { type: "strike" }]));
				break;
			case "link":
				out.push(
					...mapPhrasing(n.children, [
						...next,
						{ type: "link", attrs: { href: n.url } },
					]),
				);
				break;
			case "image":
				if (/^https?:\/\//.test(n.url)) {
					out.push(
						makeText(n.alt || n.url, [
							...next,
							{ type: "link", attrs: { href: n.url } },
						]),
					);
				} else if (n.alt) {
					out.push(makeText(n.alt, next));
				}
				break;
			case "break":
				out.push({ type: "hardBreak" });
				break;
			case "inlineMath": {
				const span = mathSpan(n.position);
				if (span === undefined || isMathText(span, n.value)) {
					out.push(
						withMarks({ type: "mathInline", attrs: { latex: n.value } }, next),
					);
					break;
				}
				/* WHY: #688 — micromark 가 통화 표기를 물었다. 왼쪽부터 탐욕적으로 짝짓기 때문에
				 * `pay $5 for $x$` 는 앞 짝을 깨야 뒤 `$x$` 가 보인다. 원문 조각을 그대로 다시
				 * 훑는다 — 형제 `text.value` 는 `\$` 가 이미 풀려 있어 쓸 수 없다(rev-691 F9). */
				let stop = span.end;
				let last = i;
				while (last + 1 < nodes.length) {
					const sibling = nodes[last + 1];
					const offset =
						sibling?.type === "text" ? sibling.position?.end.offset : undefined;
					if (offset === undefined) break;
					stop = offset;
					last++;
				}
				const region = source.slice(span.start, stop);
				/* WHY: rev-691 F9 — 백슬래시가 있으면 다시 훑지 않는다. `\$` 해석은 micromark 몫이고
				 * 정규식은 이스케이프한 달러를 구분자로 오인한다. 흡수하려던 형제는 손대지 않고
				 * 바깥 루프가 평소대로 처리하게 둔다. */
				if (region.includes("\\")) {
					const fence = "$".repeat(span.fence);
					out.push(makeText(`${fence}${n.value}${fence}`, next));
					break;
				}
				out.push(...tokenizeText(region, next, TOKEN_MATH));
				i = last;
				break;
			}
			default:
				break;
		}
	}
	return out;
}

function withHighlight(marks: Mark[], on: boolean): Mark[] {
	return on ? [...marks, { type: "highlight" }] : marks;
}

function tokenizeText(
	text: string,
	marks: Mark[],
	re: RegExp = TOKEN,
): TipNode[] {
	const out: TipNode[] = [];
	let last = 0;
	let highlight = false;
	for (const m of text.matchAll(re)) {
		const idx = m.index ?? 0;
		if (idx > last) {
			out.push(
				makeText(text.slice(last, idx), withHighlight(marks, highlight)),
			);
		}
		const refKind = m[1];
		const refId = m[2];
		const mentionKind = m[4];
		const mentionId = m[5];
		const mentionLabel = m[6];
		// WHY: #688 — TOKEN 에는 없는 그룹이라 보통 텍스트에서는 늘 undefined 다.
		const mathLatex = m[7];
		if (refKind && refId) {
			const kind = refKind as "doc" | "task" | "project";
			out.push({
				type: "mention",
				attrs: {
					entity: REF_ENTITY[kind],
					id: refId,
					label: m[3] ?? "",
				},
			});
		} else if (
			(mentionKind === "user" || mentionKind === "group") &&
			mentionId &&
			mentionLabel !== undefined
		) {
			out.push({
				type: "mention",
				attrs: { entity: mentionKind, id: mentionId, label: mentionLabel },
			});
		} else if (m[0] === "==") {
			highlight = !highlight;
		} else if (mathLatex !== undefined) {
			out.push(
				withMarks(
					{ type: "mathInline", attrs: { latex: mathLatex } },
					withHighlight(marks, highlight),
				),
			);
		}
		last = idx + m[0].length;
	}
	if (last < text.length) {
		out.push(makeText(text.slice(last), withHighlight(marks, highlight)));
	}
	return out;
}

type MathSpan = { start: number; end: number; fence: number; inner: string };

/** WHY: rev-691 — `$` 개수와 원문 안쪽을 원문에서 직접 읽는다(value 는 패딩이 벗겨져 있다). */
function mathSpan(position: Phrasing["position"]): MathSpan | undefined {
	const start = position?.start.offset;
	const end = position?.end.offset;
	if (start === undefined || end === undefined) return undefined;
	const raw = source.slice(start, end);
	const fence = /^\$+/.exec(raw)?.[0].length;
	if (fence === undefined || raw.length < fence * 2) return undefined;
	return { start, end, fence, inner: raw.slice(fence, raw.length - fence) };
}

/* WHY: rev-691 — Pandoc·GitHub 규칙. 홑 `$` 는 여는 `$` 뒤·닫는 `$` 앞이 공백이면 안 되고 닫는
 * `$` 뒤에 숫자가 오면 안 된다(`$5-$10`·`$ 5 $ 10`). `$$…$$` 는 디스플레이 수식을 문장에 옮겨
 * 적은 모양이라 숫자 절을 적용하지 않는다 — 직렬화가 그 형태로 울타리를 넓힌다. 다만 공백 절은
 * 그대로 걸어 `$$5 and $$10` 을 막는다(value 는 micromark 가 대칭 패딩을 이미 벗긴 값이다). */
function isMathText(span: MathSpan, value: string): boolean {
	if (value.trim() === "") return false;
	if (span.fence > 1) return value === value.trim();
	if (span.inner !== span.inner.trim()) return false;
	/* WHY: rev-691 — `${var}` 는 TeX 가 아니라 템플릿 리터럴이다. TeX 에서 `${x}$` 는 없는
	 * 군더더기 중괄호라, 이 하나로 리포 문서 5건의 산문 오탐이 사라진다. */
	if (span.inner.startsWith("{")) return false;
	return !/^\d/.test(source.slice(span.end, span.end + 1));
}

function withMarks(node: TipNode, marks: Mark[]): TipNode {
	const clean: Mark[] = [];
	const seen = new Set<string>();
	for (const mark of marks) {
		if (seen.has(mark.type)) continue;
		if (
			mark.type !== "bold" &&
			mark.type !== "italic" &&
			mark.type !== "strike" &&
			mark.type !== "code" &&
			mark.type !== "link" &&
			mark.type !== "highlight"
		) {
			continue;
		}
		seen.add(mark.type);
		clean.push(mark);
	}
	return clean.length > 0 ? { ...node, marks: clean } : node;
}

function makeText(text: string, marks: Mark[]): TipNode {
	return withMarks({ type: "text", text }, marks);
}
