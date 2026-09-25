// packages/editor/src/export/pdf.tsx
/* WHY: server tsx excludes workspace sources from its tsconfig. */
// @jsxRuntime automatic
import {
	Document,
	Font,
	Page,
	renderToBuffer,
	Text,
	View,
} from "@react-pdf/renderer";
import type { ReactElement } from "react";
import { emojiGlyph } from "../emoji-glyph.js";
import type { TiptapDoc } from "../json.js";
import { ExportLimitError } from "./limits.js";

/** WHY: timeoutMs 없음 — 입력 documentMaxBodyBytes(1MB) 가 시간 한도를 대신함. */
export type PdfOptions = {
	title?: string;
	fonts?: { family: string; src: Buffer }[];
	fontFamily?: string;
	fontFamilyMono?: string;
	maxOutputBytes?: number;
};

const registered = new Set<string>();

function registerFonts(fonts: { family: string; src: Buffer }[]): void {
	for (const font of fonts) {
		if (registered.has(font.family)) continue;
		Font.register({
			family: font.family,
			src: `data:font/ttf;base64,${font.src.toString("base64")}`,
		});
		registered.add(font.family);
	}
}

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

function headingSize(level: number): number {
	if (level <= 1) return 24;
	if (level === 2) return 18;
	if (level === 3) return 14;
	return 12;
}

const EMOJI_RE =
	/\p{Extended_Pictographic}(?:\u200D\p{Extended_Pictographic})*/gu;

function pushTextRuns(
	out: ReactElement[],
	text: string,
	keyBase: string,
): void {
	let last = 0;
	let i = 0;
	for (const m of text.matchAll(EMOJI_RE)) {
		const idx = m.index ?? 0;
		if (idx > last) {
			out.push(
				<Text key={`${keyBase}-${String(i++)}`}>{text.slice(last, idx)}</Text>,
			);
		}
		out.push(
			<Text
				key={`${keyBase}-${String(i++)}`}
				style={{ fontFamily: "Noto Emoji" }}
			>
				{m[0]}
			</Text>,
		);
		last = idx + m[0].length;
	}
	if (last < text.length) {
		out.push(<Text key={`${keyBase}-${String(i++)}`}>{text.slice(last)}</Text>);
	}
}

function inlineRuns(nodes: unknown[] | undefined, key = "r"): ReactElement[] {
	if (!nodes) return [];
	const out: ReactElement[] = [];
	nodes.forEach((n, i) => {
		const t = nodeType(n);
		const k = `${key}-${String(i)}`;
		if (t === "text") {
			const s = isRecord(n) && typeof n.text === "string" ? n.text : "";
			pushTextRuns(out, s, k);
			return;
		}
		if (t === "mention") {
			pushTextRuns(out, `@${strAttr(n, "label")}`, k);
			return;
		}
		// WHY: #688 — 블록 수식과 같이 LaTeX 원문을 남긴다. 아톰이라 자식 순회로는 아무것도 없다.
		if (t === "mathInline") {
			pushTextRuns(out, strAttr(n, "latex"), k);
			return;
		}
		if (t === "hardBreak") {
			out.push(<Text key={k}>{"\n"}</Text>);
			return;
		}
		if (t === "emoji") {
			out.push(
				<Text key={k} style={{ fontFamily: "Noto Emoji" }}>
					{emojiGlyph(n)}
				</Text>,
			);
			return;
		}
		out.push(...inlineRuns(nodeContent(n), k));
	});
	return out;
}

function blocks(nodes: unknown[] | undefined, mono: string): ReactElement[] {
	if (!nodes) return [];
	return nodes.map((n, i) => <View key={String(i)}>{block(n, mono)}</View>);
}

function list(n: unknown, ordered: boolean, mono: string): ReactElement {
	return (
		<View style={{ marginBottom: 6 }}>
			{(nodeContent(n) ?? []).map((item, i) => (
				<View key={String(i)} style={{ flexDirection: "row", marginBottom: 2 }}>
					<Text style={{ width: 18 }}>
						{ordered ? `${String(i + 1)}.` : "•"}
					</Text>
					<View style={{ flex: 1 }}>{blocks(nodeContent(item), mono)}</View>
				</View>
			))}
		</View>
	);
}

function table(n: unknown, mono: string): ReactElement {
	const rows = (nodeContent(n) ?? []).filter((r) => nodeType(r) === "tableRow");
	return (
		<View style={{ marginBottom: 8 }}>
			{rows.map((row, ri) => (
				<View key={String(ri)} style={{ flexDirection: "row" }}>
					{(nodeContent(row) ?? []).map((cell, ci) => (
						<View
							key={String(ci)}
							style={{
								flex: 1,
								borderWidth: 1,
								borderColor: "#ddd",
								borderStyle: "solid",
								padding: 4,
							}}
						>
							{blocks(nodeContent(cell), mono)}
						</View>
					))}
				</View>
			))}
		</View>
	);
}

function block(n: unknown, mono: string): ReactElement {
	const t = nodeType(n);
	const content = nodeContent(n);
	if (t === "attachment") {
		return <Text style={{ marginBottom: 6 }}>{strAttr(n, "name")}</Text>;
	}
	if (t === "paragraph") {
		return <Text style={{ marginBottom: 6 }}>{inlineRuns(content)}</Text>;
	}
	if (t === "heading") {
		const level = nodeAttr(n, "level");
		const nLevel =
			typeof level === "number" ? Math.min(6, Math.max(1, level)) : 1;
		return (
			<Text
				style={{
					fontSize: headingSize(nLevel),
					lineHeight: 1.5,
					fontWeight: "bold",
					marginBottom: 8,
				}}
			>
				{inlineRuns(content)}
			</Text>
		);
	}
	if (t === "blockquote") {
		return (
			<View
				style={{
					borderLeftWidth: 2,
					borderLeftColor: "#7D797A",
					borderLeftStyle: "solid",
					paddingLeft: 8,
					marginBottom: 6,
				}}
			>
				{blocks(content, mono)}
			</View>
		);
	}
	if (t === "codeBlock") {
		return (
			<Text
				style={{
					fontFamily: mono,
					fontSize: 10,
					marginBottom: 6,
				}}
			>
				{inlineRuns(content)}
			</Text>
		);
	}
	// WHY: #656 F2 — 아톰이라 말미의 blocks(content) 가 빈 View 를 냈다. LaTeX 원문을 mono 로 남긴다.
	if (t === "math") {
		return (
			<Text style={{ fontFamily: mono, fontSize: 10, marginBottom: 6 }}>
				{strAttr(n, "latex")}
			</Text>
		);
	}
	// WHY: #659 C — mermaid 도 아톰이라 같은 자리가 비었다. 다이어그램 원문을 mono 로 남긴다.
	if (t === "mermaid") {
		return (
			<Text style={{ fontFamily: mono, fontSize: 10, marginBottom: 6 }}>
				{strAttr(n, "source")}
			</Text>
		);
	}
	if (t === "bulletList") return list(n, false, mono);
	if (t === "orderedList") return list(n, true, mono);
	if (t === "listItem") return <View>{blocks(content, mono)}</View>;
	if (t === "table") return table(n, mono);
	if (t === "horizontalRule") {
		return (
			<View
				style={{
					borderBottomWidth: 1,
					borderBottomColor: "#ddd",
					borderBottomStyle: "solid",
					marginVertical: 8,
				}}
			/>
		);
	}
	return <View>{blocks(content, mono)}</View>;
}

export async function tiptapDocToPdf(
	doc: TiptapDoc,
	opts?: PdfOptions,
): Promise<Buffer> {
	const fontFamily = opts?.fontFamily ?? "Noto Sans KR";
	const fontFamilyMono = opts?.fontFamilyMono ?? "Noto Sans Mono CJK KR";
	const maxOutputBytes = opts?.maxOutputBytes ?? 20_000_000;
	if (opts?.fonts) registerFonts(opts.fonts);
	const pageFont =
		opts?.fonts && opts.fonts.length > 0 ? fontFamily : undefined;
	const document = (
		<Document title={opts?.title}>
			<Page
				size="A4"
				style={{
					paddingTop: 35,
					paddingBottom: 65,
					paddingHorizontal: 35,
					fontSize: 12,
					lineHeight: 1.5,
					...(pageFont === undefined ? {} : { fontFamily: pageFont }),
				}}
			>
				{blocks(doc.content, fontFamilyMono)}
			</Page>
		</Document>
	);
	const buffer = await renderToBuffer(document);
	if (buffer.byteLength > maxOutputBytes) {
		throw new ExportLimitError("maxOutputBytes");
	}
	return buffer;
}
