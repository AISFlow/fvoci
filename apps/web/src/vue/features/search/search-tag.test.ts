import assert from "node:assert/strict";
import test from "node:test";
import { parseSearchTagPrefix } from "./search-tag";
const tags = [
  { id: "one", name: "기획" },
  { id: "two", name: "Design" },
];
test("fixed source search tag prefix resolves Unicode/case while preserving unknown literal queries", () => {
  assert.deepEqual(parseSearchTagPrefix("  TAG:design  문서 확인  ", tags), {
    q: "문서 확인",
    tag: "two",
  });
  assert.deepEqual(parseSearchTagPrefix("tag:기획", tags), { q: "기획", tag: "one" });
  for (const q of [
    "tag:unknown hello",
    "#기획 hello",
    "hello tag:Design",
    "tag:two words",
    "plain",
  ]) {
    assert.deepEqual(parseSearchTagPrefix(q, tags), { q });
  }
});
