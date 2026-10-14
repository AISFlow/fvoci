import assert from "node:assert/strict";
import test from "node:test";
import { extractPreviewText } from "./revision-api";

await test("history preview reads Korean text in content order without interpreting markup", () => {
  assert.equal(
    extractPreviewText({
      type: "doc",
      content: [
        { type: "paragraph", content: [{ type: "text", text: "한국어 <script>" }] },
        { type: "paragraph", content: [{ type: "text", text: "다음 내용" }] },
      ],
    }),
    "한국어 <script>다음 내용",
  );
});
await test("history preview preserves explicit emoji atoms in Korean text order", () => {
  assert.equal(
    extractPreviewText({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            { type: "text", text: "복원할 한국어 본문 " },
            { type: "emoji", attrs: { emoji: "🙂" } },
            { type: "text", text: " <script>" },
          ],
        },
      ],
    }),
    "복원할 한국어 본문 🙂 <script>",
  );
});
await test("history preview resolves named emoji atoms with the maintained editor semantics", () => {
  assert.equal(
    extractPreviewText({
      type: "doc",
      content: [
        {
          type: "paragraph",
          content: [
            { type: "emoji", attrs: { name: "slightly_smiling_face" } },
            { type: "text", text: " 다음 " },
            { type: "emoji", attrs: { name: "test_tube" } },
          ],
        },
      ],
    }),
    "🙂 다음 🧪",
  );
});
await test("oversized emoji text rejects the entire preview after a valid prefix", () => {
  assert.equal(
    extractPreviewText({
      type: "doc",
      content: [
        { type: "text", text: "valid prefix" },
        { type: "emoji", attrs: { emoji: "🙂".repeat(1024 * 1024 + 1) } },
      ],
    }),
    "",
  );
});
await test("malformed, cyclic, and over-depth history cannot crash or yield partial previews", () => {
  assert.equal(extractPreviewText({ type: "doc", content: "malformed" }), "");
  const cyclic: { content: unknown[] } = { content: [] };
  cyclic.content.push(cyclic);
  assert.equal(extractPreviewText(cyclic), "");
  let deep: unknown = { type: "text", text: "too deep" };
  for (let level = 0; level < 66; level++) deep = { type: "paragraph", content: [deep] };
  assert.equal(extractPreviewText(deep), "");
  assert.equal(
    extractPreviewText({ content: [{ text: "valid prefix" }, { content: "invalid suffix" }] }),
    "",
  );
});
