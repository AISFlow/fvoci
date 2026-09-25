// packages/editor/src/export/docx.ts
import { toDocx } from "@m2d/core";
import { listPlugin } from "@m2d/list";
import { tablePlugin } from "@m2d/table";
import rehypeRaw from "rehype-raw";
import rehypeRemark from "rehype-remark";
import remarkGfm from "remark-gfm";
import remarkParse from "remark-parse";
import remarkRehype from "remark-rehype";
import { unified } from "unified";
import { ExportLimitError } from "./limits.js";

const parser = unified()
	.use(remarkParse)
	.use(remarkGfm)
	.use(remarkRehype, { allowDangerousHtml: true })
	.use(rehypeRaw)
	.use(rehypeRemark);

type MdastNode = {
	type: string;
	alt?: string | null;
	value?: string;
	children?: MdastNode[];
};

function stripRemoteImages(node: MdastNode): void {
	const kids = node.children;
	if (!kids) return;
	let i = 0;
	while (i < kids.length) {
		const child = kids[i];
		if (!child) {
			i += 1;
			continue;
		}
		if (child.type === "image" || child.type === "imageReference") {
			kids[i] = { type: "text", value: child.alt ?? "" };
			i += 1;
			continue;
		}
		if (
			child.type === "definition" ||
			(child.type === "html" && /<img\b/i.test(child.value ?? ""))
		) {
			kids.splice(i, 1);
			continue;
		}
		stripRemoteImages(child);
		i += 1;
	}
}

export async function markdownToDocx(markdown: string): Promise<Buffer> {
	const tree = await parser.run(parser.parse(markdown));
	stripRemoteImages(tree);
	const buffer = await toDocx(
		tree,
		{},
		{ plugins: [listPlugin(), tablePlugin()] },
		"nodebuffer",
	);
	if (!Buffer.isBuffer(buffer))
		throw new TypeError("DOCX serializer did not return bytes");
	if (buffer.byteLength > 20_000_000) {
		throw new ExportLimitError("maxOutputBytes");
	}
	return buffer;
}
