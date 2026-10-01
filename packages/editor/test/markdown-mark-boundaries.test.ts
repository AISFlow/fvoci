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

const linkTarget = "https://example.com/target?q=1";
const emphasisTypes = ["bold", "italic", "strike", "highlight"];
const whitespaceRuns = [" ", "  ", "\t", "\u00a0", "\u2003"];

function linkedRun(text: string, emphasis?: string, link = true): TiptapDoc {
  return {
    type: "doc",
    content: [
      {
        type: "paragraph",
        content: [
          { type: "text", text: "before " },
          {
            type: "text",
            text,
            marks: [
              ...(link ? [{ type: "link", attrs: { href: linkTarget } }] : []),
              ...(emphasis ? [{ type: emphasis }] : []),
            ],
          },
          { type: "text", text: " after" },
        ],
      },
    ],
  };
}

function parsedLinkText(doc: TiptapDoc): string {
  let text = "";
  walkTiptap(doc, (node) => {
    if (typeof node.text !== "string") return;
    const link = schema.nodeFromJSON(node).marks.find((mark) => mark.type.name === "link");
    if (!link) return;
    assert.equal(link.attrs.href, linkTarget);
    text += node.text;
  });
  return text;
}

await test("whitespace-only link labels keep href with every supported emphasis mark", () => {
  for (const emphasis of emphasisTypes) {
    for (const text of whitespaceRuns) {
      const md = tiptapDocToMd(linkedRun(text, emphasis));
      const parsed = mdToTiptapJson(md);
      assert.equal(extractText(parsed), `before ${text} after`, md);
      assert.equal(parsedLinkText(parsed), text, JSON.stringify({ emphasis, text, md, parsed }));
      walkTiptap(parsed, (node) => {
        assert.ok(
          !schema.nodeFromJSON(node).marks.some((mark) => emphasisTypes.includes(mark.type.name)),
          md,
        );
      });
    }
  }
});

await test("link-only and emphasis-only whitespace retain their existing semantics", () => {
  for (const text of whitespaceRuns) {
    const linked = mdToTiptapJson(tiptapDocToMd(linkedRun(text)));
    assert.equal(extractText(linked), `before ${text} after`);
    assert.equal(parsedLinkText(linked), text);
    for (const emphasis of emphasisTypes) {
      const input = linkedRun(text, emphasis, false);
      const parsed = mdToTiptapJson(tiptapDocToMd(input));
      assert.equal(extractText(parsed), `before ${text} after`);
      assert.equal(parsedLinkText(parsed), "");
      walkTiptap(parsed, (node) => {
        assert.equal(schema.nodeFromJSON(node).marks.length, 0);
      });
    }
  }
});

await test("linked visible emphasis keeps target, text and valid boundaries around ordinary whitespace", () => {
  for (const emphasis of emphasisTypes) {
    for (const text of [
      "한글 😀",
      " 한글 😀",
      "한글 😀 ",
      " 한글 😀 ",
      "\u00a0한글 😀\u00a0",
      "\t한글 😀\t",
    ]) {
      const md = tiptapDocToMd(linkedRun(text, emphasis));
      const parsed = mdToTiptapJson(md);
      assert.equal(extractText(parsed), `before ${text} after`, md);
      assert.equal(parsedLinkText(parsed), text, md);
      let found = false;
      walkTiptap(parsed, (node) => {
        if (typeof node.text !== "string" || !node.text.includes("한글")) return;
        found = true;
        assert.deepEqual(
          schema
            .nodeFromJSON(node)
            .marks.map((mark) => mark.type.name)
            .sort(),
          ["link", emphasis].sort(),
          md,
        );
      });
      assert.ok(found, md);
    }
  }
});

await test("code-mark whitespace links preserve target, text and code semantics", () => {
  for (const text of whitespaceRuns) {
    const md = tiptapDocToMd(linkedRun(text, "code"));
    const parsed = mdToTiptapJson(md);
    assert.equal(extractText(parsed), `before ${text} after`, md);
    assert.equal(parsedLinkText(parsed), text, md);
    walkTiptap(parsed, (node) => {
      if (typeof node.text !== "string" || !schema.nodeFromJSON(node).marks.length) return;
      assert.deepEqual(
        schema
          .nodeFromJSON(node)
          .marks.map((mark) => mark.type.name)
          .sort(),
        ["code", "link"],
      );
    });
  }
});
