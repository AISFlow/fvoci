import { z } from "zod";

export const DOCUMENT_SCHEMA_VERSION = 2;

export type TiptapDoc = { type: "doc"; content?: unknown[] };

export const tiptapDocSchema: z.ZodType<TiptapDoc> = z.object({
	type: z.literal("doc"),
	content: z.array(z.unknown()).optional(),
});

export function isTiptapDoc(value: unknown): value is TiptapDoc {
	return (
		typeof value === "object" &&
		value !== null &&
		!Array.isArray(value) &&
		"type" in value &&
		value.type === "doc" &&
		(!("content" in value) || Array.isArray(value.content))
	);
}

export function emptyDocumentJson(): TiptapDoc {
	return { type: "doc", content: [{ type: "paragraph" }] };
}
