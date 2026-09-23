// packages/editor/src/emoji-glyph.ts
import { emojis, shortcodeToEmoji } from "@tiptap/extension-emoji";

function isRecord(v: unknown): v is Record<string, unknown> {
	return typeof v === "object" && v !== null;
}

export function emojiGlyph(node: unknown): string {
	if (!isRecord(node) || !isRecord(node.attrs)) return "";
	const glyph = node.attrs.emoji;
	if (typeof glyph === "string" && glyph.length > 0) return glyph;
	const name = node.attrs.name;
	if (typeof name !== "string" || name.length === 0) return "";
	return shortcodeToEmoji(name, emojis)?.emoji ?? `:${name}:`;
}
