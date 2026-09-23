import assert from "node:assert/strict";
import test from "node:test";
import { FVOCI_YDOC_FRAGMENT } from "../src/collab/constants.ts";
import {
  replaceYDocContent,
  tiptapJsonToYDoc,
  yDocToTiptapJson,
} from "../src/collab-tiptap.ts";
import type { TiptapDoc } from "../src/json.ts";

const roundtrip = (json: TiptapDoc): TiptapDoc =>
  yDocToTiptapJson(tiptapJsonToYDoc(json, FVOCI_YDOC_FRAGMENT));

test("tiptapJsonToYDoc gc false", () => {
  const doc = tiptapJsonToYDoc({
    type: "doc",
    content: [{ type: "paragraph" }],
  });
  assert.equal(doc.gc, false);
});

test("paragraph roundtrip keeps Korean", () => {
  const json: TiptapDoc = {
    type: "doc",
    content: [
      {
        type: "paragraph",
        content: [{ type: "text", text: "안녕" }],
      },
    ],
  };
  assert.equal(JSON.stringify(roundtrip(json)).includes("안녕"), true);
});

test("replaceYDocContent", () => {
  const doc = tiptapJsonToYDoc({
    type: "doc",
    content: [
      { type: "paragraph", content: [{ type: "text", text: "old" }] },
    ],
  });
  replaceYDocContent(doc, {
    type: "doc",
    content: [
      { type: "paragraph", content: [{ type: "text", text: "new" }] },
    ],
  });
  const back = yDocToTiptapJson(doc);
  assert.equal(JSON.stringify(back).includes("new"), true);
  assert.equal(JSON.stringify(back).includes("old"), false);
});

test("ychange 는 저장 JSON 에 안 남는다", () => {
  const json = roundtrip({
    type: "doc",
    content: [
      {
        type: "paragraph",
        content: [{ type: "text", text: "a" }],
      },
    ],
  });
  assert.equal(JSON.stringify(json).includes("ychange"), false);
});
