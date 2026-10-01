import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { getSchema } from "@tiptap/core";
import { yXmlFragmentToProseMirrorRootNode } from "@tiptap/y-tiptap";
import * as Y from "yjs";
import { replaceYDocContent, tiptapJsonToYDoc, yDocToTiptapJson } from "../src/collab-tiptap.ts";
import type { TiptapDoc } from "../src/json.ts";
import { createFvociExtensions } from "../src/tiptap-schema.ts";
import { schemaCorpus } from "./schema-corpus.ts";

const schema = getSchema(createFvociExtensions());
const corpus = schemaCorpus({ user: "user-1", document: "doc-1", attachment: "file-1" });

function observeWithoutWrites(doc: Y.Doc): TiptapDoc {
  const state = Y.encodeStateAsUpdate(doc);
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
    const observed = yDocToTiptapJson(doc);
    assert.equal(updates, 0, "observation must never emit a collaboration update");
    assert.equal(changed, 0, "observation must never change a Yjs type");
    assert.deepEqual(Y.encodeStateVector(doc), vector);
    assert.ok(Y.equalSnapshots(Y.snapshot(doc), snapshot), "including deleted structs");
    assert.deepEqual(Y.encodeStateAsUpdate(doc), state);
    return observed;
  } finally {
    doc.off("update", onUpdate);
    doc.off("afterTransaction", onTransaction);
  }
}

await test("common schema corpus retains text, marks, nested tables, IDs, formula and references across fresh Y.Doc and replacement", () => {
  const expected = schema.nodeFromJSON(corpus);
  expected.check();
  const original = tiptapJsonToYDoc(corpus);
  const fresh = new Y.Doc({ gc: false });
  Y.applyUpdate(fresh, Y.encodeStateAsUpdate(original));
  assert.ok(schema.nodeFromJSON(observeWithoutWrites(fresh)).eq(expected));
  replaceYDocContent(fresh, {
    type: "doc",
    content: [{ type: "paragraph", content: [{ type: "text", text: "다른 버전" }] }],
  });
  replaceYDocContent(fresh, corpus);
  assert.ok(schema.nodeFromJSON(observeWithoutWrites(fresh)).eq(expected));
  original.destroy();
  fresh.destroy();
});

await test("existing schema seed corpus observation is read-only even with ychange and Unicode", () => {
  for (let i = 1; i <= 15; i++) {
    const names = [
      "overlapping-marks",
      "link-attrs",
      "inline-atoms",
      "tables",
      "task-lists",
      "code-blocks",
      "math-mermaid",
      "callouts-details",
      "empty",
      "empty-paragraphs",
      "unicode",
      "null-attrs",
      "nested-lists",
      "numbers",
      "ychange-and-structure",
    ];
    const name = names[i - 1];
    assert(name);
    const file = `h${String(i).padStart(2, "0")}-${name}.json`;
    const input = JSON.parse(
      readFileSync(
        new URL(`../../../compat/fixtures/yjs-seed/cases/${file}`, import.meta.url),
        "utf8",
      ),
    ) as TiptapDoc;
    const doc = tiptapJsonToYDoc(input);
    observeWithoutWrites(doc);
    doc.destroy();
  }
});

await test("schema-free observation preserves unknown node, attribute and mark without SDK repair writes", () => {
  const doc = new Y.Doc({ gc: false });
  const unknown = new Y.XmlElement("futureNode");
  unknown.setAttribute("id", "future-id");
  unknown.setAttribute("futureAttr", "미래 참조");
  const text = new Y.XmlText();
  text.insert(0, "한글 미래 😀", { futureMark: { ref: "mark-ref" } });
  unknown.insert(0, [text]);
  doc.getXmlFragment("prosemirror").insert(0, [unknown]);
  assert.deepEqual(observeWithoutWrites(doc), {
    type: "doc",
    content: [
      {
        type: "futureNode",
        attrs: { id: "future-id", futureAttr: "미래 참조" },
        content: [
          {
            type: "text",
            text: "한글 미래 😀",
            marks: [{ type: "futureMark", attrs: { ref: "mark-ref" } }],
          },
        ],
      },
    ],
  });
  // Demonstrate why the deprecated replacement is not an equivalent observer.
  // Only a disposable clone may be passed to the SDK's schema-aware repair.
  const clone = new Y.Doc({ gc: false });
  Y.applyUpdate(clone, Y.encodeStateAsUpdate(doc));
  let writes = 0;
  clone.on("update", () => writes++);
  assert.equal(
    yXmlFragmentToProseMirrorRootNode(clone.getXmlFragment("prosemirror"), schema).childCount,
    0,
  );
  assert.ok(writes > 0, "schema-aware SDK deletes unknown stored content");
  observeWithoutWrites(doc);
  clone.destroy();
  doc.destroy();
});

await test("unknown JSON nodes/marks and empty text are refused before replacing existing content", () => {
  for (const file of [
    "h16-invalid-unknown-node.json",
    "h17-invalid-empty-text.json",
    "h18-invalid-unknown-mark.json",
  ]) {
    const invalid = JSON.parse(
      readFileSync(
        new URL(`../../../compat/fixtures/yjs-seed/cases/${file}`, import.meta.url),
        "utf8",
      ),
    ) as TiptapDoc;
    assert.throws(() => {
      tiptapJsonToYDoc(invalid);
    });
    const doc = tiptapJsonToYDoc(corpus);
    const before = Y.encodeStateAsUpdate(doc);
    let writes = 0;
    doc.on("update", () => writes++);
    assert.throws(() => {
      replaceYDocContent(doc, invalid);
    });
    assert.equal(writes, 0, file);
    assert.deepEqual(Y.encodeStateAsUpdate(doc), before, file);
    assert.ok(schema.nodeFromJSON(observeWithoutWrites(doc)).eq(schema.nodeFromJSON(corpus)));
    doc.destroy();
  }
});

await test("JSON seeding uses the common schema attribute policy; raw observation retains stored extension attributes", () => {
  const doc = tiptapJsonToYDoc({
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: "known", futureAttr: "unsupported" },
        content: [{ type: "text", text: "한글" }],
      },
    ],
  });
  assert.equal(JSON.stringify(observeWithoutWrites(doc)).includes("futureAttr"), false);
  const paragraph = doc.getXmlFragment("prosemirror").get(0);
  assert.ok(paragraph instanceof Y.XmlElement);
  paragraph.setAttribute("futureAttr", "already stored");
  assert.equal(JSON.stringify(observeWithoutWrites(doc)).includes("already stored"), true);
  doc.destroy();
});
