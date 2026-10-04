import assert from "node:assert/strict";
import test from "node:test";
import { getSchema } from "@tiptap/core";
import * as Y from "yjs";
import { tiptapJsonToYDoc, yDocToTiptapJson } from "../src/collab-tiptap.ts";
import { extractInternalRefs } from "../src/extract.ts";
import type { TiptapDoc } from "../src/json.ts";
import { createFvociExtensions } from "../src/tiptap-schema.ts";
import { schemaCorpus } from "./schema-corpus.ts";
import {
  absent,
  corpusRefs,
  rawPresenceFixture,
  v050ContractCorpus,
} from "./v050-contract-corpus.ts";
import type { CorpusNode, CorpusValue, SemanticFact } from "./v050-contract-corpus.ts";

const schema = getSchema(createFvociExtensions());

// Only JavaScript object prototypes disappear here; no JSON field, ID, attr,
// reference, Unicode code point or missing/present distinction is rewritten.
function semanticValue(value: unknown): unknown {
  if (typeof value !== "object" || value === null) return value;
  const result: object = Array.isArray(value) ? new Array<unknown>(value.length) : {};
  for (const key of Reflect.ownKeys(value)) {
    if (!Object.prototype.propertyIsEnumerable.call(value, key)) continue;
    Object.defineProperty(result, key, {
      value: semanticValue(Reflect.get(value, key)),
      enumerable: true,
      configurable: true,
      writable: true,
    });
  }
  return result;
}

function atPath(value: unknown, path: string): unknown {
  for (const part of path.split(".")) {
    assert.ok(typeof value === "object" && value !== null, `missing parent of ${path}`);
    if (!Object.hasOwn(value, part)) return absent;
    value = Reflect.get(value, part);
  }
  return value;
}

function assertFacts(observed: unknown, facts: SemanticFact[]): void {
  const normalized = semanticValue(observed);
  for (const fact of facts)
    assert.deepEqual(atPath(normalized, fact.path), semanticValue(fact.value), fact.path);
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
  const result = new Y.XmlElement<Record<string, Exclude<CorpusValue, undefined>>>(node.type);
  for (const [key, value] of Object.entries(node.attrs ?? {})) {
    if (value === undefined) {
      // Pinned Yjs ValueTypes omits top-level undefined. This raw regression
      // exercises actual setAttribute/wire/observation, with no value coercion.
      // @ts-expect-error -- bounded raw undefined attr is retained by real Yjs
      result.setAttribute(key, value);
    } else result.setAttribute(key, value);
  }
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
        assert.deepEqual(semanticValue(observeWithoutWrites(fresh)), semanticValue(fixture.input));
      const invalid = fixture.invalidInput;
      if (invalid) assert.throws(() => tiptapJsonToYDoc(invalid));
    } finally {
      original.destroy();
      fresh.destroy();
    }
  });
}

await test("raw own undefined, null and absent attributes survive original and fresh Y.Doc observation", () => {
  const original = rawDocument(rawPresenceFixture.input);
  const fresh = new Y.Doc({ gc: false });
  try {
    Y.applyUpdate(fresh, Y.encodeStateAsUpdate(original));
    for (const stored of [original, fresh]) {
      const observed = observeWithoutWrites(stored);
      assertFacts(observed, rawPresenceFixture.expected);
      assert.deepEqual(semanticValue(observed), semanticValue(rawPresenceFixture.input));
      for (const [path, changes] of [
        ["content.0.attrs.undefinedAttr", [absent, null]],
        ["content.0.attrs.nullAttr", [absent, undefined]],
        ["content.0.attrs.absentAttr", [undefined, null]],
        ["content.0.attrs.nested.undefinedValue", [absent, null]],
        ["content.0.attrs.nested.nullValue", [absent, undefined]],
        ["content.0.attrs.nested.values.0", [absent, null]],
        ["content.0.attrs.nested.values.1", [absent, undefined]],
        ["content.0.attrs.nested.absentValue", [undefined, null]],
      ] as const) {
        for (const changed of changes) {
          const corrupted = structuredClone(observed);
          changePath(corrupted, path, changed);
          assert.throws(() => {
            assert.deepEqual(semanticValue(corrupted), semanticValue(rawPresenceFixture.input));
          }, path);
          assert.throws(() => {
            assertFacts(corrupted, rawPresenceFixture.expected);
          }, path);
        }
      }
    }
  } finally {
    original.destroy();
    fresh.destroy();
  }
});

function changePath(value: unknown, path: string, changed: unknown): void {
  const parts = path.split(".");
  const last = parts.pop();
  assert.ok(last);
  const parent = atPath(value, parts.join("."));
  assert.ok(typeof parent === "object" && parent !== null);
  if (changed === absent) assert.ok(Reflect.deleteProperty(parent, last));
  else assert.ok(Reflect.set(parent, last, changed));
}

await test("every existing block, entity and attachment ID has a handwritten fact rejecting replacement and deletion", () => {
  for (const fixture of v050ContractCorpus) {
    const stored =
      fixture.storage === "schema" ? tiptapJsonToYDoc(fixture.input) : rawDocument(fixture.input);
    try {
      const observed = observeWithoutWrites(stored);
      function visit(node: CorpusNode, path: string): void {
        if (node.attrs && Object.hasOwn(node.attrs, "id")) {
          const idPath = `${path}.attrs.id`;
          const fact = fixture.expected.find((item) => item.path === idPath);
          // Validate authored inventory against input, never generate expected
          // semantics from the converter/observer. The domains stay separate:
          // mention=entity, attachment=file, other ID-bearing nodes=block.
          assert.ok(fact, `${fixture.id}: missing handwritten ${node.type} ID at ${idPath}`);
          assert.deepEqual(fact.value, node.attrs.id);
          for (const changed of ["replacement-id", absent]) {
            const corrupted = structuredClone(observed);
            changePath(corrupted, idPath, changed);
            assert.throws(() => {
              assertFacts(corrupted, fixture.expected);
            }, `${fixture.id}: ${idPath}`);
          }
        }
        node.content?.forEach((child, index) => {
          visit(child, `${path}.content.${String(index)}`);
        });
      }
      (fixture.input.content as CorpusNode[]).forEach((node, index) => {
        visit(node, `content.${String(index)}`);
      });
    } finally {
      stored.destroy();
    }
  }
});

await test("F12 absence, duplicate block IDs and file-domain collision do not allocate or conflate IDs", () => {
  const fixture = v050ContractCorpus.find((item) => item.id === "F12");
  assert.ok(fixture);
  const stored = tiptapJsonToYDoc(fixture.input);
  try {
    const observed = observeWithoutWrites(stored);
    assertFacts(observed, fixture.expected);
    for (const changed of [
      undefined,
      null,
      {},
      { id: undefined },
      { id: null },
      { id: "allocated" },
    ]) {
      const corrupted = structuredClone(observed);
      changePath(corrupted, "content.0.attrs", changed);
      assert.throws(() => {
        assertFacts(corrupted, fixture.expected);
      });
    }
    // Neither duplicate is repaired; the identical file UUID belongs to a
    // different domain from the paragraph's block ID. No SDK editor is mounted.
    assert.equal(atPath(observed, "content.0.attrs.id"), absent);
    assert.equal(atPath(observed, "content.1.attrs.id"), "duplicate");
    assert.equal(atPath(observed, "content.2.attrs.id"), "duplicate");
    assert.equal(atPath(observed, "content.3.type"), "paragraph");
    assert.equal(atPath(observed, "content.4.type"), "attachment");
  } finally {
    stored.destroy();
  }
});

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

await test("same-ID text loss and added marks cannot pass the named inline semantics", () => {
  for (const [id, path, changed] of [
    ["F01", "content.1.content.0.text", "lost research"],
    [
      "F01",
      "content.1.content.2.marks",
      [
        { type: "code", attrs: {} },
        { type: "bold", attrs: {} },
      ],
    ],
    ["F02", "content.0.content.0.text", "lost color text"],
    ["F03", "content.0.content.0.content.0.content.0.marks", [{ type: "bold", attrs: {} }]],
    ["F05", "content.0.content.0.marks", [{ type: "bold", attrs: {} }]],
    ["F09", "content.0.content.0.marks", [{ type: "bold", attrs: {} }]],
    ["F11", "content.0.content.0.marks", [{ type: "bold", attrs: {} }]],
    ["F12", "content.0.content.0.marks", [{ type: "bold", attrs: {} }]],
  ] as const) {
    const fixture = v050ContractCorpus.find((item) => item.id === id);
    assert.ok(fixture);
    const stored = tiptapJsonToYDoc(fixture.input);
    try {
      const corrupted = structuredClone(observeWithoutWrites(stored));
      changePath(corrupted, path, changed);
      assert.throws(() => {
        assertFacts(corrupted, fixture.expected);
      }, `${id}: ${path}`);
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
    for (const loss of fixture.losses) assert.notEqual(atPath(fixture.input, loss.path), absent);
  }
});
