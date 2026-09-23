import assert from "node:assert/strict";
import test from "node:test";
import { extractInternalRefs, extractText, toChosung } from "../src/extract.ts";

test("toChosung", () => {
  assert.equal(toChosung("한글"), "ㅎㄱ");
});

test("extractInternalRefs keeps mention and embed uuid refs", () => {
  const docId = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";
  const taskId = "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb";
  const json = {
    type: "doc",
    content: [
      {
        type: "paragraph",
        content: [
          {
            type: "mention",
            attrs: { entity: "document", id: docId, label: "김희" },
          },
          {
            type: "embed",
            attrs: { entity: "task", ref: taskId },
          },
        ],
      },
    ],
  };
  assert.equal(extractText(json).includes("김희"), true);
  assert.deepEqual(extractInternalRefs(json), [
    { kind: "document", id: docId },
    { kind: "task", id: taskId },
  ]);
});

test("extractInternalRefs drops non-uuid ids", () => {
  const json = {
    type: "doc",
    content: [
      {
        type: "mention",
        attrs: { entity: "document", id: "not-a-uuid", label: "x" },
      },
    ],
  };
  assert.deepEqual(extractInternalRefs(json), []);
});
