import { uuid } from "./uuid.js";
import { emojiGlyph } from "./emoji-glyph.js";
import type { TiptapDoc } from "./json.js";
import type { Extensions, JSONContent } from "@tiptap/core";
import { generateUniqueIds } from "@tiptap/extension-unique-id";

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
          typeof row === "object" && row !== null && "content" in row && Array.isArray(row.content)
            ? row.content
            : [];
        return cells.map((cell) => tiptapText(cell, depth + 2)).join(" ");
      })
      .join("\n");
  }
  const parts = n.content.map((child) => tiptapText(child, depth + 1)).filter((s) => s.length > 0);
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
  visit: (n: TiptapWalkNode, depth: number) => void,
  depth = 0,
): void {
  if (depth > TIPTAP_WALK_MAX_DEPTH) return;
  if (typeof node !== "object" || node === null) return;
  const n = node as TiptapWalkNode;
  visit(n, depth);
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

const UNIQUE_ID_NODE_TYPE_SET: ReadonlySet<string> = new Set(UNIQUE_ID_NODE_TYPES);

/** A new document owns new block identities; reference entity IDs stay intact.
 * The caller supplies the maintained current editor extensions/schema. */
export function independentDraftBody(doc: TiptapDoc, extensions: Extensions): TiptapDoc {
  // Match the actual REST derived-body byte cap before SDK schema conversion.
  if (new TextEncoder().encode(JSON.stringify(doc)).length > 1024 * 1024)
    throw new Error("Independent draft body exceeds limit");
  const next = structuredClone(doc);
  let count = 0;
  walkTiptap(next, (node, depth) => {
    if (++count > 20_000 || (depth === TIPTAP_WALK_MAX_DEPTH && node.content?.length))
      throw new Error("Independent draft structure exceeds limit");
    if (typeof node.type === "string" && UNIQUE_ID_NODE_TYPE_SET.has(node.type) && node.attrs)
      delete node.attrs.id;
  });
  // The pinned SDK validates this unknown-content DTO with Node.fromJSON using
  // the current shared schema before assigning IDs; unknown nodes still throw.
  return generateUniqueIds(next as JSONContent, extensions) as TiptapDoc;
}

export function replaceTiptapNodeById(
  doc: TiptapDoc,
  blockId: string,
  node: TiptapWalkNode,
): TiptapDoc | null {
  const next = structuredClone(doc);
  // The visitor mutates this result; an object keeps callback writes visible to TypeScript.
  const result = { hit: false };
  walkTiptap(next, (n) => {
    if (result.hit) return;
    if (n.attrs?.id !== blockId) return;
    if (typeof n.type !== "string" || !UNIQUE_ID_NODE_TYPE_SET.has(n.type)) {
      return;
    }
    result.hit = true;
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
  return result.hit ? next : null;
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
      if (entity === "document" || entity === "task") add(entity, str(n.attrs?.id));
    }
    if (n.type === "embed") {
      const entity = str(n.attrs?.entity);
      if (entity === "document" || entity === "task") add(entity, str(n.attrs?.ref));
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
