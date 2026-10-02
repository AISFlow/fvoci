import assert from "node:assert/strict";
import test from "node:test";
import { getSchema } from "@tiptap/core";
import * as Y from "yjs";
import { tiptapJsonToYDoc, yDocToTiptapJson } from "../src/collab-tiptap.ts";
import { extractInternalRefs } from "../src/extract.ts";
import type { TiptapDoc } from "../src/json.ts";
import { createFvociExtensions } from "../src/tiptap-schema.ts";
import { schemaCorpus } from "./schema-corpus.ts";
import { corpusRefs, v050ContractCorpus } from "./v050-contract-corpus.ts";
import type { CorpusNode, CorpusValue, SemanticFact } from "./v050-contract-corpus.ts";

const schema = getSchema(createFvociExtensions());

// Only JavaScript object prototypes disappear here; no JSON field, ID, attr,
// reference, Unicode code point or missing/present distinction is rewritten.
function jsonValue(value: unknown): unknown {
  return JSON.parse(JSON.stringify(value)) as unknown;
}

function atPath(value: unknown, path: string): unknown {
  for (const part of path.split(".")) {
    assert.ok(typeof value === "object" && value !== null, `missing parent of ${path}`);
    value = Reflect.get(value, part);
  }
  return value;
}

function assertFacts(observed: unknown, facts: SemanticFact[]): void {
  for (const fact of facts)
    assert.deepEqual(atPath(jsonValue(observed), fact.path), fact.value, fact.path);
}

// A test-only raw stored-data builder, deliberately bypassing schema coercion.
// Schema validity and already-stored extension preservation are separate checks.
function rawNode(node: CorpusNode): Y.XmlElement | Y.XmlText {
  if (node.type === "text") {
    const result = new Y.XmlText();
    result.insert(
      0,
      node.text ?? "",
      Object.fromEntries((node.marks ?? []).map((mark) => [mark.type, mark.attrs ?? {}])),
    );
    return result;
  }
  const result = new Y.XmlElement<Record<string, CorpusValue>>(node.type);
  for (const [key, value] of Object.entries(node.attrs ?? {})) result.setAttribute(key, value);
  result.insert(0, (node.content ?? []).map(rawNode));
  // Yjs insert() declares the default string-attribute element type even
  // though its constructor/setAttribute generic supports these JSON attrs.
  // This test-only boundary inserts nodes; it never reads attrs as strings.
  return result as Y.XmlElement;
}

function rawDocument(input: TiptapDoc): Y.Doc {
  const result = new Y.Doc({ gc: false });
  result.getXmlFragment("prosemirror").insert(0, (input.content as CorpusNode[]).map(rawNode));
  return result;
}

// Same observation boundary as schema-observation.test.ts: no SDK repair,
// no projection normalization, and deleted structs participate in the guard.
function observeWithoutWrites(doc: Y.Doc): TiptapDoc {
  const before = Y.encodeStateAsUpdate(doc);
  const vector = Y.encodeStateVector(doc);
  const snapshot = Y.snapshot(doc);
  let updates = 0;
  let changed = 0;
  const onUpdate = () => updates++;
  const onTransaction = (tr: Y.Transaction) => {
    changed += tr.changed.size;
  };
  doc.on("update", onUpdate);
  doc.on("afterTransaction", onTransaction);
  try {
    const result = yDocToTiptapJson(doc);
    assert.equal(updates, 0);
    assert.equal(changed, 0);
    assert.deepEqual(Y.encodeStateVector(doc), vector);
    assert.ok(Y.equalSnapshots(Y.snapshot(doc), snapshot));
    assert.deepEqual(Y.encodeStateAsUpdate(doc), before);
    return result;
  } finally {
    doc.off("update", onUpdate);
    doc.off("afterTransaction", onTransaction);
  }
}

for (const fixture of v050ContractCorpus) {
  await test(`${fixture.id}: handwritten semantic facts survive read-only storage observation (${fixture.storage})`, () => {
    if (fixture.storage === "schema") schema.nodeFromJSON(fixture.input).check();
    const original =
      fixture.storage === "schema" ? tiptapJsonToYDoc(fixture.input) : rawDocument(fixture.input);
    const fresh = new Y.Doc({ gc: false });
    try {
      Y.applyUpdate(fresh, Y.encodeStateAsUpdate(original));
      assertFacts(observeWithoutWrites(original), fixture.expected);
      assertFacts(observeWithoutWrites(fresh), fixture.expected);
      if (fixture.storage === "raw")
        assert.deepEqual(jsonValue(observeWithoutWrites(fresh)), fixture.input);
      const invalid = fixture.invalidInput;
      if (invalid) assert.throws(() => tiptapJsonToYDoc(invalid));
    } finally {
      original.destroy();
      fresh.destroy();
    }
  });
}

await test("existing schemaCorpus remains a reusable handwritten ingredient", () => {
  const input = schemaCorpus(corpusRefs);
  schema.nodeFromJSON(input).check();
  const stored = tiptapJsonToYDoc(input);
  try {
    assertFacts(observeWithoutWrites(stored), [
      { path: "content.0.attrs.id", value: "corpus-heading" },
      { path: "content.0.content.0.text", value: "한국어 문서 😀" },
      { path: "content.3.content.0.content.1.content.1.attrs.id", value: "corpus-inner-table" },
      { path: "content.5.attrs.id", value: "10000000-0000-4000-8000-000000000006" },
    ]);
  } finally {
    stored.destroy();
  }
});

await test("reference occurrences are ordered; backlinks deduplicate only document/task targets", () => {
  const fixture = v050ContractCorpus.find((item) => item.id === "F07");
  assert.ok(fixture);
  const stored = tiptapJsonToYDoc(fixture.input);
  try {
    const observed = observeWithoutWrites(stored);
    const occurrences: unknown[] = [];
    function visit(node: CorpusNode) {
      if (node.type === "mention")
        occurrences.push(["mention", node.attrs?.entity, node.attrs?.id, node.attrs?.label]);
      if (node.type === "embed") occurrences.push(["embed", node.attrs?.entity, node.attrs?.ref]);
      node.content?.forEach(visit);
    }
    (observed.content as CorpusNode[]).forEach(visit);
    assert.deepEqual(occurrences, [
      ["mention", "user", "10000000-0000-4000-8000-000000000001", "같은 이름"],
      ["mention", "group", "10000000-0000-4000-8000-000000000002", "같은 이름"],
      ["mention", "document", "10000000-0000-4000-8000-000000000003", "같은 이름"],
      ["mention", "task", "10000000-0000-4000-8000-000000000004", "같은 이름"],
      ["mention", "project", "10000000-0000-4000-8000-000000000005", "같은 이름"],
      ["mention", "task", "10000000-0000-4000-8000-000000000004", "같은 이름"],
      ["embed", "document", "10000000-0000-4000-8000-000000000003"],
      ["embed", "task", "10000000-0000-4000-8000-000000000004"],
      ["embed", "project", "10000000-0000-4000-8000-000000000005"],
      ["embed", "url", "https://example.com/source"],
    ]);
    assert.deepEqual(extractInternalRefs(observed), [
      { kind: "document", id: "10000000-0000-4000-8000-000000000003" },
      { kind: "task", id: "10000000-0000-4000-8000-000000000004" },
    ]);
  } finally {
    stored.destroy();
  }
});

await test("raw extension facts detect schema coercion rather than normalize it away", () => {
  const future = v050ContractCorpus.find((item) => item.id === "F10");
  const table = v050ContractCorpus.find((item) => item.id === "F04");
  assert.ok(future && table);
  assert.throws(() => schema.nodeFromJSON(future.input));
  const coerced = schema.nodeFromJSON(table.input);
  coerced.check();
  assert.throws(() => {
    assertFacts(coerced.toJSON(), table.expected);
  });
});

await test("details atom rejection and read-only raw preservation are separate contracts", () => {
  const fixture = v050ContractCorpus.find((item) => item.id === "F06");
  assert.ok(fixture);
  const stored = rawDocument(fixture.input);
  const before = Y.encodeStateAsUpdate(stored);
  try {
    assert.throws(() => {
      schema.nodeFromJSON(fixture.input).check();
    }, /Invalid content for node detailsSummary/);
    assert.deepEqual(Y.encodeStateAsUpdate(stored), before);
    assertFacts(observeWithoutWrites(stored), fixture.expected);
    // A separate handwritten supported sibling: summary contains marked text.
    schema
      .nodeFromJSON({
        type: "doc",
        content: [
          {
            type: "details",
            attrs: { id: "supported-details", open: true },
            content: [
              {
                type: "detailsSummary",
                attrs: { id: "supported-summary" },
                content: [{ type: "text", text: "요약", marks: [{ type: "bold" }] }],
              },
              {
                type: "detailsContent",
                attrs: { id: "supported-content" },
                content: [
                  {
                    type: "paragraph",
                    attrs: { id: "supported-p" },
                    content: [{ type: "text", text: "내용" }],
                  },
                ],
              },
            ],
          },
        ],
      })
      .check();
  } finally {
    stored.destroy();
  }
});

await test("semantic facts reject identity, reference, Unicode and layout corruption", () => {
  for (const [id, path, changed] of [
    ["F01", "content.0.attrs.id", "new-id"],
    ["F01", "content.0.content.0.text", "한글 🧑💻 ❤"],
    ["F07", "content.2.attrs.ref", "10000000-0000-4000-8000-000000000099"],
    ["F08", "content.0.attrs.previewWidth", 320],
  ] as const) {
    const fixture = v050ContractCorpus.find((item) => item.id === id);
    assert.ok(fixture);
    const stored = tiptapJsonToYDoc(fixture.input);
    try {
      const corrupted = structuredClone(observeWithoutWrites(stored));
      const parts = path.split(".");
      const last = parts.pop();
      assert.ok(last);
      const parent = atPath(corrupted, parts.join("."));
      assert.ok(typeof parent === "object" && parent !== null);
      Reflect.set(parent, last, changed);
      assert.throws(() => {
        assertFacts(corrupted, fixture.expected);
      });
    } finally {
      stored.destroy();
    }
  }
});

await test("all twelve bounded cases carry explicit loss and required-behavior expectations", () => {
  assert.deepEqual(
    v050ContractCorpus.map((item) => item.id),
    ["F01", "F02", "F03", "F04", "F05", "F06", "F07", "F08", "F09", "F10", "F11", "F12"],
  );
  for (const fixture of v050ContractCorpus) {
    let blocks = 0;
    let units = 0;
    function visit(node: CorpusNode, depth: number) {
      assert.ok(depth <= 9, `${fixture.id}: nesting`);
      if (
        node.type !== "text" &&
        node.type !== "hardBreak" &&
        node.type !== "mention" &&
        node.type !== "mathInline" &&
        node.type !== "emoji"
      )
        blocks++;
      units += node.text?.length ?? 0;
      if (node.type === "table") {
        assert.ok((node.content?.length ?? 0) <= 3);
        for (const row of node.content ?? []) assert.ok((row.content?.length ?? 0) <= 3);
      }
      node.content?.forEach((child) => {
        visit(child, depth + 1);
      });
    }
    (fixture.input.content as CorpusNode[]).forEach((node) => {
      visit(node, 1);
    });
    assert.ok(blocks <= 32);
    assert.ok(units <= 8192);
    assert.ok(
      JSON.stringify(fixture.input).length + (fixture.sourceExamples ?? []).join("\n").length <=
        8192,
    );
    assert.ok(
      fixture.expected.length > 0 && fixture.losses.length > 0 && fixture.required.length > 0,
    );
    for (const loss of fixture.losses) assert.notEqual(atPath(fixture.input, loss.path), undefined);
  }
});
