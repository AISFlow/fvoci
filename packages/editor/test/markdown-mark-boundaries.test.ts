import assert from "node:assert/strict";
import test from "node:test";
import { getSchema } from "@tiptap/core";
import { createFvociExtensions } from "../src/tiptap-schema.ts";
import { mdToTiptapJson } from "../src/markdown/parse.ts";
import { tiptapDocToMd } from "../src/md.ts";
import { extractText, walkTiptap } from "../src/extract.ts";
import type { TiptapDoc } from "../src/json.ts";

const schema = getSchema(createFvociExtensions());

await test("Markdown preserves visible emphasis and boundary whitespace for Unicode and mixed marks", () => {
  for (const text of [
    "한글 😀",
    "한글 😀 ",
    " 한글 😀",
    " 한글 😀 ",
    "\u00a0한글 😀\u00a0",
    "\t한글 😀\t",
  ]) {
    for (const marks of [
      [{ type: "bold" }],
      [{ type: "italic" }],
      [{ type: "bold" }, { type: "italic" }],
      [{ type: "strike" }],
    ]) {
      const input: TiptapDoc = {
        type: "doc",
        content: [
          {
            type: "paragraph",
            content: [
              { type: "text", text: "앞 " },
              { type: "text", text, marks },
              { type: "text", text: " 뒤" },
            ],
          },
        ],
      };
      const imported = mdToTiptapJson(tiptapDocToMd(input));
      assert.equal(extractText(imported), `앞 ${text} 뒤`);
      let found = false;
      walkTiptap(imported, (node) => {
        if (typeof node.text !== "string" || !node.text.includes("한글")) return;
        found = true;
        assert.deepEqual(
          schema.nodeFromJSON(node).marks.map((mark): unknown => mark.toJSON()),
          schema
            .nodeFromJSON({ type: "text", text, marks })
            .marks.map((mark): unknown => mark.toJSON()),
          JSON.stringify({ text, marks, imported }),
        );
      });
      assert.ok(found);
    }
  }
});

await test("all-whitespace emphasis does not emit broken delimiters, while code padding and math keep their semantics", () => {
  for (const text of [" ", "  ", "\t", "\u00a0"]) {
    const input: TiptapDoc = {
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            { type: "text", text: "앞" },
            { type: "text", text, marks: [{ type: "bold" }] },
            { type: "text", text: "뒤" },
          ],
        },
      ],
    };
    assert.equal(extractText(mdToTiptapJson(tiptapDocToMd(input))), `앞${text}뒤`);
  }
  const mixed: TiptapDoc = {
    type: "doc",
    content: [
      {
        type: "paragraph",
        content: [
          { type: "text", text: "앞 " },
          { type: "text", text: " code $x$ ", marks: [{ type: "code" }, { type: "bold" }] },
          { type: "text", text: " 수식 " },
          { type: "mathInline", attrs: { latex: "x^2" } },
          { type: "text", text: " 끝" },
        ],
      },
    ],
  };
  const imported = mdToTiptapJson(tiptapDocToMd(mixed));
  assert.equal(extractText(imported), "앞  code $x$  수식 x^2 끝");
  const json = JSON.stringify(imported);
  assert.ok(json.includes('"code"'));
  assert.ok(json.includes('"bold"'));
  assert.ok(json.includes('"mathInline"'));
});
