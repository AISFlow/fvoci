import { describe, expect, test } from "bun:test";
import { independentDraftBody, UNIQUE_ID_NODE_TYPES, walkTiptap } from "../src/extract";
import { createFvociExtensions } from "../src/tiptap-schema";
import type { TiptapDoc } from "../src/json";
import { uuid } from "../src/uuid";

const extensions = createFvociExtensions();
const attachment = "11111111-1111-4111-8111-111111111111";
const person = "22222222-2222-4222-8222-222222222222";
const document = "33333333-3333-4333-8333-333333333333";
const body: TiptapDoc = {
  type: "doc",
  content: [
    {
      type: "heading",
      attrs: { id: "old-heading", level: 2 },
      content: [{ type: "text", text: "제목" }],
    },
    {
      type: "paragraph",
      attrs: { id: "old-paragraph" },
      content: [
        { type: "text", text: "private 😀", marks: [{ type: "bold" }] },
        { type: "mention", attrs: { id: person, label: "member", entity: "user" } },
      ],
    },
    {
      type: "attachment",
      attrs: { id: attachment, name: "literal.txt", mime: "text/plain", size: 3 },
    },
    { type: "embed", attrs: { id: "old-embed-block", entity: "document", ref: document } },
  ],
};
const ids = (doc: TiptapDoc) => {
  const out: string[] = [];
  walkTiptap(doc, (node) => {
    if (UNIQUE_ID_NODE_TYPES.includes(node.type as (typeof UNIQUE_ID_NODE_TYPES)[number]))
      out.push(String(node.attrs?.id));
  });
  return out;
};

describe("independent OFF draft block identity through maintained UniqueID SDK", () => {
  test("fresh block IDs preserve references, literal text, marks and the original body", () => {
    const original = structuredClone(body);
    const next = independentDraftBody(body, extensions);
    const fresh = ids(next);
    expect(body).toEqual(original);
    expect(fresh).toHaveLength(3);
    expect(new Set(fresh).size).toBe(3);
    for (const id of fresh) {
      expect(ids(original)).not.toContain(id);
      expect(uuid.safeParse(id).success).toBe(true);
    }
    const json = JSON.stringify(next);
    expect(json).toContain(attachment);
    expect(json).toContain(person);
    expect(json).toContain(document);
    expect(json).toContain('"entity":"user"');
    expect(json).toContain('"entity":"document"');
    expect(json).toContain("private 😀");
    expect(json).toContain('"bold"');
    expect(json).toContain('"level":2');
  });
  test("separate logical documents get separate block IDs and missing IDs are filled", () => {
    const missing: TiptapDoc = {
      type: "doc",
      content: [{ type: "paragraph", content: [{ type: "text", text: "new" }] }],
    };
    const first = independentDraftBody(missing, extensions);
    const second = independentDraftBody(missing, extensions);
    expect(ids(first)).toHaveLength(1);
    expect(ids(second)).toHaveLength(1);
    expect(ids(first)[0]).not.toBe(ids(second)[0]);
    expect(missing.content?.[0]).toEqual({
      type: "paragraph",
      content: [{ type: "text", text: "new" }],
    });
  });
  test("unknown nodes are rejected by the actual shared schema rather than silently copied", () => {
    expect(() =>
      independentDraftBody(
        { type: "doc", content: [{ type: "unregistered-private-node" }] },
        extensions,
      ),
    ).toThrow();
  });
  test("oversized UTF8 bodies and over-depth trees refuse before SDK conversion", () => {
    expect(() =>
      independentDraftBody(
        {
          type: "doc",
          content: [{ type: "paragraph", content: [{ type: "text", text: "한".repeat(400_000) }] }],
        },
        extensions,
      ),
    ).toThrow("body exceeds");
    let node: unknown = { type: "paragraph", content: [{ type: "text", text: "deep" }] };
    for (let depth = 0; depth < 65; depth++) node = { type: "blockquote", content: [node] };
    expect(() => independentDraftBody({ type: "doc", content: [node] }, extensions)).toThrow(
      "structure exceeds",
    );
  });
});
