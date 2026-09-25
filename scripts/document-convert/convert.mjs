#!/usr/bin/env node
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { isTiptapDoc } from "@fvoci/editor/json";
import { mdToTiptapJson } from "@fvoci/editor/markdown/parse";
import { tiptapDocToMd } from "@fvoci/editor/md";
import { tiptapJsonToYUpdate } from "@fvoci/editor/collab-tiptap";
import { markdownToDocx } from "@fvoci/editor/export/docx";
import { tiptapDocToPdf } from "@fvoci/editor/export/pdf";
import { tiptapDocToPptx } from "@fvoci/editor/export/pptx";
import { ExportLimitError } from "@fvoci/editor/export/limits";

const ROOT = dirname(fileURLToPath(import.meta.url));
const FONT_DIR = join(ROOT, "../../packages/editor/src/fonts");
const MAX_OUTPUT_BYTES = 20_000_000;

function readFonts() {
	const names = [
		["Noto Sans KR", "NotoSansKR.ttf"],
		["Noto Sans Mono CJK KR", "NotoSansMonoCJKkr.ttf"],
		["Noto Emoji", "NotoEmoji.ttf"],
	];
	return names.map(([family, file]) => ({
		family,
		src: readFileSync(join(FONT_DIR, file)),
	}));
}

function visibleTitle(title) {
	return title.replace(/[\r\n\u2028\u2029]+/g, " ").trim();
}

function titledDocument(title, doc) {
	const text = visibleTitle(title);
	if (!text) return doc;
	return {
		...doc,
		content: [
			{ type: "heading", attrs: { level: 1 }, content: [{ type: "text", text }] },
			...(doc.content ?? []),
		],
	};
}

function documentMarkdown(title, doc) {
	const text = visibleTitle(title).replace(/[!-/:-@[-`{-~]/g, "\\$&");
	const body = tiptapDocToMd(doc);
	return text ? `# ${text}\n\n${body}` : body;
}

function respond(ok, body) {
	process.stdout.write(JSON.stringify({ ok, ...body }));
}

async function main() {
	// The request always arrives on stdin (no argv/env size limits). Read it
	// as a stream: readFileSync(0) fails with EAGAIN on a pipe that is not
	// yet filled.
	const chunks = [];
	for await (const chunk of process.stdin) chunks.push(chunk);
	const raw = Buffer.concat(chunks).toString("utf8");
	const req = JSON.parse(raw);
	const op = req.op;
	try {
		if (op === "md_to_tiptap") {
			const contentJson = mdToTiptapJson(req.markdown ?? "");
			return respond(true, { contentJson });
		}
		if (op === "tiptap_to_yjs_update") {
			if (!isTiptapDoc(req.contentJson)) {
				return respond(false, { code: "invalid_input" });
			}
			const update = tiptapJsonToYUpdate(req.contentJson);
			return respond(true, { updateB64: Buffer.from(update).toString("base64") });
		}
		if (!isTiptapDoc(req.contentJson)) {
			return respond(false, { code: "invalid_input" });
		}
		const title = req.title ?? "export";
		const doc = req.contentJson;
		let buffer;
		let contentType;
		let ext;
		if (op === "export_md") {
			buffer = Buffer.from(documentMarkdown(title, doc), "utf8");
			contentType = "text/markdown; charset=utf-8";
			ext = "md";
		} else if (op === "export_pdf") {
			const fonts = readFonts();
			buffer = await tiptapDocToPdf(titledDocument(title, doc), {
				title,
				fonts,
				fontFamily: "Noto Sans KR",
				fontFamilyMono: "Noto Sans Mono CJK KR",
				maxOutputBytes: MAX_OUTPUT_BYTES,
			});
			contentType = "application/pdf";
			ext = "pdf";
		} else if (op === "export_docx") {
			buffer = await markdownToDocx(documentMarkdown(title, doc), {
				maxOutputBytes: MAX_OUTPUT_BYTES,
			});
			contentType =
				"application/vnd.openxmlformats-officedocument.wordprocessingml.document";
			ext = "docx";
		} else if (op === "export_pptx") {
			buffer = await tiptapDocToPptx(titledDocument(title, doc), {
				title,
				maxOutputBytes: MAX_OUTPUT_BYTES,
			});
			contentType =
				"application/vnd.openxmlformats-officedocument.presentationml.presentation";
			ext = "pptx";
		} else {
			return respond(false, { code: "invalid_input" });
		}
		if (buffer.byteLength > MAX_OUTPUT_BYTES) {
			return respond(false, { code: "document_body_exceeds_document_max_body_bytes" });
		}
		return respond(true, {
			contentType,
			ext,
			dataB64: buffer.toString("base64"),
		});
	} catch (err) {
		if (err instanceof ExportLimitError) {
			return respond(false, { code: "document_body_exceeds_document_max_body_bytes" });
		}
		return respond(false, { code: "invalid_input", detail: String(err?.message ?? err) });
	}
}

main().catch((err) => {
	respond(false, { code: "invalid_input", detail: String(err?.message ?? err) });
	process.exit(1);
});
