import assert from "node:assert/strict";
import { test } from "node:test";
import { readFileSync } from "node:fs";
import { compareRevisionProjections, type RevisionProjection } from "./revision-diff.ts";

// The pure comparison accepts the existing editor's block ID contract; it
// does not add a schema or confuse attachment/mention resource IDs with it.
const extract = readFileSync(
  new URL("../../../../../packages/editor/src/extract.ts", import.meta.url),
  "utf8",
);
const declaration = extract.match(/export const UNIQUE_ID_NODE_TYPES = \[([\s\S]*?)\] as const;/);
assert.ok(declaration);
const blockTypes = JSON.parse(`[${(declaration[1] ?? "").replace(/,\s*$/, "")}]`) as string[];
const paragraph = (id: string | null, text: string, marks?: unknown[]) => ({
  type: "paragraph",
  ...(id === null ? {} : { attrs: { id } }),
  content: [{ type: "text", text, ...(marks ? { marks } : {}) }],
});
function projection(id: string, content: unknown[]): RevisionProjection {
  return {
    id,
    targetKind: "document",
    targetId: "same-document",
    contentJson: { type: "doc", content },
  };
}
const cell = (id: string, text: string) => ({
  type: "tableCell",
  attrs: { colspan: 1, rowspan: 1 },
  content: [paragraph(id, text)],
});
const row = (cells: unknown[]) => ({ type: "tableRow", content: cells });
const table = (rows: unknown[]) => ({ type: "table", attrs: { id: "table-1" }, content: rows });
const taskList = (checked: boolean) => ({
  type: "taskList",
  attrs: { id: "list-1" },
  content: [
    {
      type: "taskItem",
      attrs: { id: "check-1", checked },
      content: [paragraph("check-text", "논문 검토")],
    },
  ],
});
const ref = (id: string) => ({
  type: "paragraph",
  attrs: { id: "p-reference" },
  content: [{ type: "mention", attrs: { entity: "document", id, label: "관련 문서" } }],
});
const fileA = "11111111-1111-4111-8111-111111111111";
const fileB = "22222222-2222-4222-8222-222222222222";
const before = projection("R-before", [
  paragraph("p-intro", "한글 공부 계획"),
  paragraph("p-move", "자료 읽기"),
  paragraph("p-remove", "삭제할 메모"),
  taskList(false),
  table([
    row([cell("head-a", "자료"), cell("head-b", "예상")]),
    row([cell("data-a", "논문"), cell("data-b", "30분")]),
  ]),
  paragraph("p-link", "원자료", [{ type: "link", attrs: { href: "https://example.test/old" } }]),
  { type: "attachment", attrs: { id: fileA, name: "연구.pdf", image: false } },
  ref("doc-ref"),
  paragraph(null, "이전 기록"),
]);
const after = projection("R-after", [
  paragraph("p-move", "자료 읽기"),
  paragraph("p-intro", "한글 연구 계획"),
  paragraph("p-add", "새 메모"),
  taskList(true),
  table([
    row([cell("head-a", "자료"), cell("head-b", "예상")]),
    row([cell("data-a", "논문"), cell("data-b", "45분")]),
    row([cell("new-a", "책"), cell("new-b", "20분")]),
  ]),
  paragraph("p-link", "원자료", [{ type: "link", attrs: { href: "https://example.test/new" } }]),
  { type: "attachment", attrs: { id: fileB, name: "수정.pdf", image: false } },
  ref("doc-next"),
  paragraph(null, "이전 기록 보완"),
]);

await test("literal Korean corpus distinguishes blocks, text, checkbox, table, links, files and references on one frozen pair", () => {
  const originalBefore = structuredClone(before);
  const originalAfter = structuredClone(after);
  const diff = compareRevisionProjections(before, after, blockTypes);
  assert.equal(diff.beforeId, "R-before");
  assert.equal(diff.afterId, "R-after");
  const identified = (kind: string) =>
    diff.changes
      .filter((change) => change.kind === kind)
      .map((change) => (change.after ?? change.before)?.blockId);
  assert.deepEqual(
    identified("moved"),
    ["p-move"],
    "relative reorder; unchanged intro is not another moved block",
  );
  assert.ok(identified("removed").includes("p-remove"));
  assert.ok(identified("added").includes("p-add"));
  const intro = diff.changes.find(
    (change) => change.kind === "text" && change.after?.blockId === "p-intro",
  );
  assert.ok(intro);
  assert.equal(intro.before?.text, "한글 공부 계획");
  assert.equal(intro.after?.text, "한글 연구 계획");
  const check = diff.changes.find((change) => change.kind === "checkbox");
  assert.deepEqual(check?.values, { before: [false], after: [true] });
  assert.ok(identified("table").includes("table-1"), "added row is a table structure change");
  assert.equal(
    diff.changes.find((change) => change.kind === "text" && change.after?.blockId === "data-b")
      ?.after?.text,
    "45분",
  );
  assert.ok(identified("added").includes("new-a"));
  assert.ok(identified("added").includes("new-b"));
  const link = diff.changes.find((change) => change.kind === "link");
  assert.deepEqual(link?.values, {
    before: [{ path: [0], text: "원자료", attrs: { href: "https://example.test/old" } }],
    after: [{ path: [0], text: "원자료", attrs: { href: "https://example.test/new" } }],
  });
  const file = diff.changes.find((change) => change.kind === "attachment");
  assert.equal(file?.identity, "position", "file UUID is a resource, not stable block identity");
  assert.deepEqual(file.values, {
    before: [{ id: fileA, image: false, name: "연구.pdf" }],
    after: [{ id: fileB, image: false, name: "수정.pdf" }],
  });
  const reference = diff.changes.find((change) => change.kind === "reference");
  assert.deepEqual(reference?.values, {
    before: [
      {
        path: [0],
        type: "mention",
        attrs: { entity: "document", id: "doc-ref", label: "관련 문서" },
      },
    ],
    after: [
      {
        path: [0],
        type: "mention",
        attrs: { entity: "document", id: "doc-next", label: "관련 문서" },
      },
    ],
  });
  assert.ok(diff.limits.includes("missing-ids"));
  assert.ok(diff.limits.includes("positional-match"));
  const idless = diff.changes.find(
    (change) => change.kind === "text" && change.after?.text === "이전 기록 보완",
  );
  assert.equal(idless?.identity, "position");
  assert.equal(idless.before?.blockId, null);
  assert.deepEqual(before, originalBefore, "before projection remains untouched");
  assert.deepEqual(after, originalAfter, "after projection remains untouched");
});

await test("insertion/deletion shifts do not mark every surviving block moved", () => {
  const diff = compareRevisionProjections(
    projection("A", [paragraph("a", "가"), paragraph("b", "나")]),
    projection("B", [paragraph("new", "신규"), paragraph("a", "가"), paragraph("b", "나")]),
    blockTypes,
  );
  assert.deepEqual(
    diff.changes.map((change) => [change.kind, change.after?.blockId]),
    [["added", "new"]],
  );
});
await test("stable nested block moved to another identified parent is a move, not delete/add", () => {
  const quote = (id: string, content: unknown[]) => ({
    type: "blockquote",
    attrs: { id },
    content,
  });
  const diff = compareRevisionProjections(
    projection("A", [quote("left", [paragraph("p", "이동")]), quote("right", [])]),
    projection("B", [quote("left", []), quote("right", [paragraph("p", "이동")])]),
    blockTypes,
  );
  assert.equal(
    diff.changes.filter((change) => change.kind === "moved" && change.after?.blockId === "p")
      .length,
    1,
  );
  assert.equal(
    diff.changes.filter((change) => ["added", "removed"].includes(change.kind)).length,
    0,
  );
});
await test("duplicate IDs refuse exact matching rather than inventing identity", () => {
  const diff = compareRevisionProjections(
    projection("A", [paragraph("duplicate", "가"), paragraph("duplicate", "나")]),
    projection("B", [paragraph("duplicate", "다")]),
    blockTypes,
  );
  assert.ok(diff.limits.includes("duplicate-ids"));
  assert.equal(
    diff.changes.some((change) => change.identity === "block-id"),
    false,
  );
});
await test("later IDs do not reconstruct past missing IDs", () => {
  const diff = compareRevisionProjections(
    projection("A", [paragraph(null, "같은 글")]),
    projection("B", [paragraph("new-id", "같은 글")]),
    blockTypes,
  );
  assert.deepEqual(
    diff.changes.map((change) => change.kind),
    ["removed", "added"],
  );
  assert.ok(diff.limits.includes("missing-ids"));
});
await test("repeated unidentified blocks remain explicitly uncertain and never exact moves", () => {
  const diff = compareRevisionProjections(
    projection("A", [paragraph(null, "동일"), paragraph(null, "동일")]),
    projection("B", [paragraph(null, "동일"), paragraph(null, "수정")]),
    blockTypes,
  );
  assert.ok(diff.limits.includes("missing-ids"));
  assert.equal(
    diff.changes.some((change) => change.kind === "moved"),
    false,
  );
  assert.equal(
    diff.changes.every((change) => change.identity === "position"),
    true,
  );
});
await test("same semantic object/mark-set with different serialization order has no diff", () => {
  const a = {
    type: "paragraph",
    attrs: { id: "p", textAlign: "center" },
    content: [{ type: "text", text: "한글", marks: [{ type: "bold" }, { type: "italic" }] }],
  };
  const b = {
    content: [{ marks: [{ type: "italic" }, { type: "bold" }], text: "한글", type: "text" }],
    attrs: { textAlign: "center", id: "p" },
    type: "paragraph",
  };
  assert.deepEqual(
    compareRevisionProjections(projection("A", [a]), projection("B", [b]), blockTypes).changes,
    [],
  );
});
await test("different targets/kinds are never a valid pair", () => {
  for (const invalid of [
    { ...after, targetId: "other-document" },
    { ...after, targetKind: "task" },
  ])
    assert.throws(() => compareRevisionProjections(before, invalid, blockTypes), /same target/);
});
await test("malformed and excessive-depth content is disclosed without a partial exact result", () => {
  const invalid = compareRevisionProjections(
    before,
    { ...after, contentJson: { type: "doc", content: [null] } },
    blockTypes,
  );
  assert.deepEqual(invalid.changes, []);
  assert.ok(invalid.limits.includes("invalid-content"));
  let deep: unknown = paragraph("inner", "내용");
  for (let i = 0; i < 70; i++) deep = { type: "blockquote", content: [deep] };
  const limited = compareRevisionProjections(before, projection("deep", [deep]), blockTypes);
  assert.deepEqual(limited.changes, []);
  assert.deepEqual(limited.limits, ["size-limit"]);
});

await test("adjacent same-format text runs do not fabricate a content change", () => {
  const a = paragraph("p", "한글 문장", [{ type: "bold" }]);
  const b = {
    type: "paragraph",
    attrs: { id: "p" },
    content: [
      { type: "text", text: "한글 ", marks: [{ type: "bold" }] },
      { type: "text", text: "문장", marks: [{ type: "bold" }] },
    ],
  };
  assert.deepEqual(
    compareRevisionProjections(projection("A", [a]), projection("B", [b]), blockTypes).changes,
    [],
  );
});
await test("unknown attributes are observable and excessive attribute nesting/cycles are bounded", () => {
  const a = { ...paragraph("p", "같은 글"), attrs: { id: "p", extra: { policy: "before" } } };
  const b = { ...a, attrs: { id: "p", extra: { policy: "after" } } };
  assert.equal(
    compareRevisionProjections(projection("A", [a]), projection("B", [b]), blockTypes).changes[0]
      ?.kind,
    "attributes",
  );
  let attr: unknown = {};
  for (let i = 0; i < 200; i++) attr = { deeper: attr };
  assert.deepEqual(
    compareRevisionProjections(
      projection("A", [a]),
      projection("B", [{ ...b, attrs: { id: "p", attr } }]),
      blockTypes,
    ).limits,
    ["size-limit"],
  );
  const cyclic: { type: string; content: unknown[] } = { type: "doc", content: [] };
  cyclic.content.push(cyclic);
  assert.deepEqual(
    compareRevisionProjections(before, { ...after, contentJson: cyclic }, blockTypes).limits,
    ["invalid-content"],
  );
});

await test("root metadata and residual unknown fields remain observable alongside named changes", () => {
  const old = { ...paragraph("p", "이전 글"), futurePolicy: { disclosure: "이전 정책" } };
  const next = { ...paragraph("p", "다음 글"), futurePolicy: { disclosure: "다음 정책" } };
  const a = {
    ...projection("A", [old]),
    contentJson: { type: "doc", revisionPolicy: "원본", content: [old] },
  };
  const b = {
    ...projection("B", [next]),
    contentJson: { type: "doc", revisionPolicy: "변경", content: [next] },
  };
  const diff = compareRevisionProjections(a, b, blockTypes);
  assert.ok(
    diff.changes.some(
      (change) =>
        change.kind === "text" &&
        change.before?.text === "이전 글" &&
        change.after?.text === "다음 글",
    ),
  );
  assert.ok(
    diff.changes.some(
      (change) =>
        change.kind === "attributes" &&
        change.before?.type === "doc" &&
        JSON.stringify(change.values?.before).includes("원본") &&
        JSON.stringify(change.values?.after).includes("변경"),
    ),
  );
  const residual = diff.changes.find(
    (change) => change.kind === "attributes" && change.after?.blockId === "p",
  );
  assert.deepEqual(residual?.values, {
    before: [{ path: [], fields: { futurePolicy: { disclosure: "이전 정책" } } }],
    after: [{ path: [], fields: { futurePolicy: { disclosure: "다음 정책" } } }],
  });
  assert.equal(
    compareRevisionProjections(
      { ...a, contentJson: { type: "doc", attrs: { policy: "private" }, content: [] } },
      { ...b, contentJson: { type: "doc", attrs: { policy: "shared" }, content: [] } },
      blockTypes,
    ).changes[0]?.kind,
    "attributes",
  );
});
