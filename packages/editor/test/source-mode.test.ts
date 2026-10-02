import assert from "node:assert/strict";
import test from "node:test";
import { getSchema } from "@tiptap/core";
import { EditorState } from "@tiptap/pm/state";
import * as Y from "yjs";
import { tiptapJsonToYDoc, yDocToTiptapJson } from "../src/collab-tiptap.ts";
import type { TiptapDoc } from "../src/json.ts";
import { rawEditorPreflight, SourceModeSession } from "../src/source-mode.ts";
import { createFvociExtensions } from "../src/tiptap-schema.ts";
import { type CorpusNode, type CorpusValue, v050ContractCorpus } from "./v050-contract-corpus.ts";

const schema = getSchema(createFvociExtensions());
const paragraph = (id: string, text: string) => ({
  type: "paragraph",
  attrs: { id },
  content: [{ type: "text", text }],
});
const document = (...content: unknown[]): TiptapDoc => ({ type: "doc", content });

for (const kind of ["plain", "uniform", "later-color", "different-runs", "link"] as const) {
  await test(`paragraph join ${kind} preserves independently declared full inline semantics and neighbors`, () => {
    const color = { type: "textStyle", attrs: { color: "#112233" } };
    const underline = { type: "underline" };
    const link = {
      type: "link",
      attrs: {
        href: "https://example.com/a%20b?q=한글#target",
        title: "exact title",
        target: "_self",
      },
    };
    const firstMarks = kind === "uniform" ? [color] : kind === "different-runs" ? [underline] : [];
    const laterMarks = kind === "plain" ? [] : kind === "link" ? [color, link] : [color];
    const fixture = setup(
      document(
        paragraph("outside-before", "untouched"),
        {
          type: "paragraph",
          attrs: { id: "join-first" },
          content: [{ type: "text", text: "alpha", marks: firstMarks }],
        },
        {
          type: "paragraph",
          attrs: { id: "join-later" },
          content: [
            { type: "text", text: "beta ", marks: kind === "uniform" ? [color] : [] },
            { type: "text", text: "감마 🧑‍💻", marks: laterMarks },
          ],
        },
        {
          type: "embed",
          attrs: {
            id: "outside-ref",
            entity: "document",
            ref: "10000000-0000-4000-8000-000000000003",
          },
        },
        {
          type: "attachment",
          attrs: {
            id: "10000000-0000-4000-8000-000000000006",
            name: "exact 한글.pdf",
            mime: "application/pdf",
            size: 17,
          },
        },
      ),
    );
    try {
      const capture = fixture.session.capture(fixture.state.doc);
      const before = Y.encodeStateAsUpdate(fixture.ydoc);
      const source = capture.source.replace("alpha\n\nbeta", "alpha beta");
      assert.notEqual(source, capture.source);
      const proposal = fixture.session.prepare(capture, source, fixture.state);
      assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
      assert.ok(proposal.transaction);
      const after = fixture.state.apply(proposal.transaction).doc;
      assert.equal(after.childCount, 4);
      const joined = after.child(1);
      assert.equal(joined.attrs.id, "join-first");
      assert.equal(joined.textContent, "alpha beta 감마 🧑‍💻");
      const marked = joined.content.content.find((run) => run.text?.includes("감마"));
      assert.ok(marked);
      assert.equal(
        marked.marks.find((mark) => mark.type.name === "textStyle")?.attrs.color,
        kind === "plain" ? undefined : "#112233",
      );
      const alpha = joined.child(0);
      assert.equal(
        alpha.marks.some((mark) => mark.type.name === "underline"),
        kind === "different-runs",
      );
      assert.equal(
        alpha.marks.find((mark) => mark.type.name === "textStyle")?.attrs.color,
        kind === "uniform" ? "#112233" : undefined,
      );
      if (kind === "link") {
        const actual = marked.marks.find((mark) => mark.type.name === "link");
        assert.ok(actual);
        for (const [field, expected] of Object.entries(link.attrs))
          assert.equal(actual.attrs[field], expected);
      }
      assert.ok(after.child(0).eq(fixture.state.doc.child(0)));
      assert.ok(after.child(2).eq(fixture.state.doc.child(3)));
      assert.ok(after.child(3).eq(fixture.state.doc.child(4)));
      assert.deepEqual(
        Y.encodeStateAsUpdate(fixture.ydoc),
        before,
        "preparation is observation only",
      );
    } finally {
      fixture.close();
    }
  });
}

for (const field of ["textAlign", "futureFlag"] as const) {
  await test(`paragraph join refuses a later distinct ${field} with precise loss and zero-write Cancel`, () => {
    const fixture = setup(
      document(
        paragraph("outside", "neighbor"),
        paragraph("join-first", "alpha"),
        paragraph("join-later", "beta gamma"),
      ),
    );
    const later = fixture.ydoc.getXmlFragment("prosemirror").get(2);
    assert.ok(later instanceof Y.XmlElement);
    later.setAttribute(field, field === "textAlign" ? "right" : "preserve");
    const state = EditorState.create({
      schema,
      doc: schema.nodeFromJSON(yDocToTiptapJson(fixture.ydoc)),
    });
    const manager = new Y.UndoManager(fixture.ydoc.getXmlFragment("prosemirror"));
    let updates = 0;
    fixture.ydoc.on("update", () => updates++);
    try {
      const capture = fixture.session.capture(state.doc);
      const before = Y.encodeStateAsUpdate(fixture.ydoc);
      const proposal = fixture.session.prepare(
        capture,
        capture.source.replace("alpha\n\nbeta", "alpha beta"),
        state,
      );
      assert.equal(proposal.status, "loss");
      assert.equal(proposal.transaction, undefined);
      assert.equal(proposal.diagnostics[0]?.id, "join-later");
      assert.equal(proposal.diagnostics[0].path, "document.content.2");
      assert.equal(proposal.diagnostics[0].field, `attrs.${field}`);
      fixture.session.destroy(); // Cancel retires the private proposal, never compensates with a write.
      assert.equal(updates, 0);
      assert.equal(manager.undoStack.length, 0);
      assert.deepEqual(Y.encodeStateAsUpdate(fixture.ydoc), before);
      assert.equal(later.getAttribute(field), field === "textAlign" ? "right" : "preserve");
    } finally {
      manager.destroy();
      fixture.close();
    }
  });
}

await test("paragraph join permits identical presentation attrs on every consumed block", () => {
  const fixture = setup(
    document(
      ...["alpha", "beta", "gamma"].map((text, index) => ({
        ...paragraph(`uniform-${String(index)}`, text),
        attrs: { id: `uniform-${String(index)}`, textAlign: "right" },
      })),
    ),
  );
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const proposal = fixture.session.prepare(capture, "alpha beta gamma", fixture.state);
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.ok(proposal.transaction);
    assert.equal(proposal.transaction.doc.childCount, 1);
    assert.equal(proposal.transaction.doc.child(0).attrs.textAlign, "right");
    assert.equal(proposal.transaction.doc.child(0).attrs.id, "uniform-0");
    assert.equal(proposal.transaction.doc.textContent, "alpha beta gamma");
  } finally {
    fixture.close();
  }
});

await test("joined replacement refuses hidden metadata on a later run rather than inheriting the first run", () => {
  const fixture = setup(
    document(paragraph("outside", "neighbor"), paragraph("join-first", "alpha"), {
      type: "paragraph",
      attrs: { id: "join-later" },
      content: [
        { type: "text", text: "beta " },
        {
          type: "text",
          text: "gamma",
          marks: [{ type: "textStyle", attrs: { color: "#112233" } }],
        },
      ],
    }),
  );
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const before = Y.encodeStateAsUpdate(fixture.ydoc);
    const proposal = fixture.session.prepare(
      capture,
      "neighbor\n\nALPHA BETA GAMMA",
      fixture.state,
    );
    assert.equal(proposal.status, "loss");
    assert.equal(proposal.transaction, undefined);
    assert.equal(proposal.diagnostics[0]?.id, "join-later");
    assert.equal(proposal.diagnostics[0].path, "document.content.2");
    assert.equal(proposal.diagnostics[0].field, "content/marks/attrs");
    assert.deepEqual(Y.encodeStateAsUpdate(fixture.ydoc), before);
  } finally {
    fixture.close();
  }
});

await test("joined replacement of plain uniform runs stays directly editable", () => {
  const fixture = setup(document(paragraph("first", "alpha"), paragraph("later", "beta gamma")));
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const proposal = fixture.session.prepare(capture, "ALPHA BETA GAMMA", fixture.state);
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.ok(proposal.transaction);
    assert.equal(proposal.transaction.doc.textContent, "ALPHA BETA GAMMA");
    assert.equal(proposal.transaction.doc.child(0).attrs.id, "first");
  } finally {
    fixture.close();
  }
});

function setup(input: TiptapDoc) {
  const ydoc = tiptapJsonToYDoc(input);
  const state = EditorState.create({ schema, doc: schema.nodeFromJSON(yDocToTiptapJson(ydoc)) });
  const session = new SourceModeSession(
    ydoc,
    () => 1,
    () => true,
  );
  return {
    ydoc,
    state,
    session,
    close: () => {
      session.destroy();
      ydoc.destroy();
    },
  };
}

function rawNode(node: CorpusNode): Y.XmlElement | Y.XmlText {
  if (node.type === "text") {
    const text = new Y.XmlText();
    text.insert(
      0,
      node.text ?? "",
      Object.fromEntries((node.marks ?? []).map((mark) => [mark.type, mark.attrs ?? {}])),
    );
    return text;
  }
  const element = new Y.XmlElement<Record<string, Exclude<CorpusValue, undefined>>>(node.type);
  for (const [key, value] of Object.entries(node.attrs ?? {})) {
    if (value === undefined) {
      // @ts-expect-error -- real Yjs retains the bounded raw own undefined attr
      element.setAttribute(key, value);
    } else element.setAttribute(key, value);
  }
  element.insert(0, (node.content ?? []).map(rawNode));
  return element as Y.XmlElement;
}

for (const fixture of v050ContractCorpus) {
  await test(`${fixture.id}: source viewing/no-op and Cancel are schema-free zero-write observations`, () => {
    const ydoc =
      fixture.storage === "schema" ? tiptapJsonToYDoc(fixture.input) : new Y.Doc({ gc: false });
    if (fixture.storage === "raw")
      ydoc
        .getXmlFragment("prosemirror")
        .insert(0, (fixture.input.content as CorpusNode[]).map(rawNode));
    const state = EditorState.create({
      schema,
      doc:
        fixture.storage === "schema"
          ? schema.nodeFromJSON(yDocToTiptapJson(ydoc))
          : (schema.topNodeType.createAndFill() ?? undefined),
    });
    const session = new SourceModeSession(
      ydoc,
      () => "actor1",
      () => true,
    );
    const before = Y.encodeStateAsUpdate(ydoc);
    const snapshot = Y.snapshot(ydoc);
    const manager = new Y.UndoManager(ydoc.getXmlFragment("prosemirror"));
    let updates = 0;
    ydoc.on("update", () => updates++);
    try {
      const capture = session.capture(state.doc);
      const preflight = rawEditorPreflight(ydoc, schema);
      if (fixture.id === "F10") {
        assert.ok(preflight.some((item) => item.field === "type" && item.id === "f10-future"));
        assert.ok(preflight.some((item) => item.field === "marks.futureMark"));
      } else if (fixture.id === "F06")
        assert.ok(preflight.some((item) => item.field === "content/schema"));
      else assert.equal(preflight.length, 0, fixture.id);
      assert.equal(session.prepare(capture, capture.source, state).status, "noop");
      session.prepare(capture, `${capture.source}\n\nchanged`, state);
      // Cancel is disposal of a proposal; never a compensating document write.
      session.destroy();
      assert.equal(updates, 0);
      assert.equal(manager.undoStack.length, 0);
      assert.ok(Y.equalSnapshots(Y.snapshot(ydoc), snapshot));
      assert.deepEqual(Y.encodeStateAsUpdate(ydoc), before);
    } finally {
      manager.destroy();
      session.destroy();
      ydoc.destroy();
    }
  });
}

await test("localized Korean text edit retains block ID, opaque sibling, reference and attachment tuples", () => {
  const fixture = setup(
    document(
      paragraph("body", "한글 연구 🧑‍💻"),
      {
        type: "attachment",
        attrs: { id: "file-1", name: "자료.pdf", width: 70, align: "right", caption: "설명" },
      },
      { type: "embed", attrs: { id: "ref-1", entity: "task", ref: "task-1" } },
    ),
  );
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const proposal = fixture.session.prepare(
      capture,
      capture.source.replace("연구", "조사"),
      fixture.state,
    );
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.ok(proposal.transaction);
    const after = fixture.state.apply(proposal.transaction);
    assert.equal(after.doc.child(0).attrs.id, "body");
    assert.equal(after.doc.child(0).textContent, "한글 조사 🧑‍💻");
    assert.ok(after.doc.child(1).eq(fixture.state.doc.child(1)));
    assert.ok(after.doc.child(2).eq(fixture.state.doc.child(2)));
    const step = proposal.transaction.steps[0]?.toJSON() as { from: number; to: number };
    assert.equal(step.from, 4);
    assert.equal(step.to, 6);
    assert.equal(proposal.transaction.steps.length, 1);
  } finally {
    fixture.close();
  }
});

await test("changing a marked run preserves hidden underline/color and link metadata", () => {
  const fixture = setup(
    document({
      type: "paragraph",
      attrs: { id: "p", textAlign: "right" },
      content: [
        {
          type: "text",
          text: "자료",
          marks: [
            { type: "underline" },
            { type: "textStyle", attrs: { color: "#112233" } },
            {
              type: "link",
              attrs: { href: "https://example.com", title: "연구", target: "_self" },
            },
          ],
        },
      ],
    }),
  );
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const proposal = fixture.session.prepare(
      capture,
      capture.source.replace("자료", "노트"),
      fixture.state,
    );
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.ok(proposal.transaction);
    const after = fixture.state.apply(proposal.transaction).doc.child(0);
    assert.equal(after.attrs.textAlign, "right");
    assert.deepEqual(after.child(0).marks, fixture.state.doc.child(0).child(0).marks);
  } finally {
    fixture.close();
  }
});

await test("nested list edit and task checkbox preserve every mapped existing ID", () => {
  const fixture = setup(
    document({
      type: "taskList",
      attrs: { id: "list" },
      content: [
        {
          type: "taskItem",
          attrs: { id: "item", checked: false },
          content: [paragraph("p", "연구")],
        },
      ],
    }),
  );
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const proposal = fixture.session.prepare(
      capture,
      capture.source.replace("[ ]", "[x]").replace("연구", "조사"),
      fixture.state,
    );
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.ok(proposal.transaction);
    const after = fixture.state.apply(proposal.transaction).doc.child(0);
    assert.equal(after.attrs.id, "list");
    assert.equal(after.child(0).attrs.id, "item");
    assert.equal(after.child(0).attrs.checked, true);
    assert.equal(after.child(0).child(0).attrs.id, "p");
    assert.equal(after.textContent, "조사");
  } finally {
    fixture.close();
  }
});

await test("table cell text edit preserves geometry, cell kinds and paragraph IDs", () => {
  const fixture = setup(
    document({
      type: "table",
      attrs: { id: "table" },
      content: [
        {
          type: "tableRow",
          content: [
            {
              type: "tableHeader",
              attrs: { colspan: 2, rowspan: 1, colwidth: [120, 180], backgroundColor: "#abcdef" },
              content: [paragraph("cell", "자료")],
            },
          ],
        },
      ],
    }),
  );
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const proposal = fixture.session.prepare(
      capture,
      capture.source.replace("자료", "노트"),
      fixture.state,
    );
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.ok(proposal.transaction);
    const after = fixture.state.apply(proposal.transaction).doc.child(0);
    assert.deepEqual(
      after.child(0).child(0).attrs,
      fixture.state.doc.child(0).child(0).child(0).attrs,
    );
    assert.equal(after.child(0).child(0).child(0).attrs.id, "cell");
    assert.equal(after.textContent, "노트");
  } finally {
    fixture.close();
  }
});

await test("deletion-only remote update changes epoch even with identical state vector", () => {
  const fixture = setup(document(paragraph("p", "연구")));
  const peer = new Y.Doc({ gc: false });
  try {
    Y.applyUpdate(peer, Y.encodeStateAsUpdate(fixture.ydoc));
    const capture = fixture.session.capture(fixture.state.doc);
    const vector = Y.encodeStateVector(fixture.ydoc);
    peer.getXmlFragment("prosemirror").delete(0, 1);
    Y.applyUpdate(fixture.ydoc, Y.encodeStateAsUpdate(peer, vector));
    assert.deepEqual(Y.encodeStateVector(fixture.ydoc), vector);
    assert.equal(fixture.session.isCurrent(capture), false);
    assert.equal(
      fixture.session.prepare(capture, capture.source.replace("연구", "조사"), fixture.state)
        .status,
      "stale",
    );
  } finally {
    peer.destroy();
    fixture.close();
  }
});

await test("scope ABA needs a monotonically advanced host scope and retirement rejects late proposals", () => {
  const fixture = setup(document(paragraph("p", "연구")));
  let scope = 1;
  const session = new SourceModeSession(
    fixture.ydoc,
    () => scope,
    () => true,
  );
  try {
    const capture = session.capture(fixture.state.doc);
    scope = 2;
    scope = 3;
    assert.equal(session.prepare(capture, "조사", fixture.state).status, "stale");
    const fresh = session.capture(fixture.state.doc);
    session.destroy();
    assert.equal(session.isCurrent(fresh), false);
  } finally {
    session.destroy();
    fixture.close();
  }
});

await test("missing/duplicate IDs reject changes without viewing-time repair or file-domain collision", () => {
  const fixture = v050ContractCorpus.find((item) => item.id === "F12");
  assert.ok(fixture);
  const current = setup(fixture.input);
  try {
    const capture = current.session.capture(current.state.doc);
    const before = Y.encodeStateAsUpdate(current.ydoc);
    const proposal = current.session.prepare(capture, `${capture.source}\nchanged`, current.state);
    assert.equal(proposal.status, "loss");
    assert.ok(proposal.diagnostics.some((item) => item.field === "id"));
    assert.deepEqual(Y.encodeStateAsUpdate(current.ydoc), before);
  } finally {
    current.close();
  }
});

await test("reference loss warning identifies the changed node and Cancel cannot mutate it", () => {
  const fixture = setup(
    document({ type: "embed", attrs: { id: "ref", entity: "url", ref: "https://example.com" } }),
  );
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const before = Y.encodeStateAsUpdate(fixture.ydoc);
    const proposal = fixture.session.prepare(capture, "https://example.org", fixture.state);
    assert.equal(proposal.status, "loss");
    assert.equal(proposal.diagnostics[0]?.id, "ref");
    assert.deepEqual(Y.encodeStateAsUpdate(fixture.ydoc), before);
  } finally {
    fixture.close();
  }
});

await test("partial Markdown emphasis edit preserves every untouched color and Unicode run", () => {
  const fixture = setup(
    document({
      type: "paragraph",
      attrs: { id: "p" },
      content: [
        {
          type: "text",
          text: "한글 자료",
          marks: [{ type: "underline" }, { type: "textStyle", attrs: { color: "#112233" } }],
        },
      ],
    }),
  );
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const proposal = fixture.session.prepare(capture, "한글 **자료**", fixture.state);
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.ok(proposal.transaction);
    const after = fixture.state.apply(proposal.transaction).doc.child(0);
    assert.equal(after.textContent, "한글 자료");
    assert.equal(after.childCount, 2);
    assert.deepEqual(after.child(0).marks, fixture.state.doc.child(0).child(0).marks);
    assert.equal(
      after.child(1).marks.find((mark) => mark.type.name === "textStyle")?.attrs.color,
      "#112233",
    );
    assert.ok(after.child(1).marks.some((mark) => mark.type.name === "underline"));
    assert.ok(after.child(1).marks.some((mark) => mark.type.name === "bold"));
  } finally {
    fixture.close();
  }
});

await test("existing mention identity/label and attachment file domain survive edits alongside their generated tokens", () => {
  const fixture = setup(
    document({
      type: "paragraph",
      attrs: { id: "p" },
      content: [
        { type: "text", text: "자료 " },
        { type: "mention", attrs: { entity: "user", id: "user-1", label: "동료" } },
        { type: "text", text: " 연구" },
      ],
    }),
  );
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    assert.ok(capture.source.includes("user-1"));
    const proposal = fixture.session.prepare(
      capture,
      capture.source.replace("연구", "조사"),
      fixture.state,
    );
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.ok(proposal.transaction);
    const after = fixture.state.apply(proposal.transaction).doc.child(0);
    assert.equal(after.attrs.id, "p");
    assert.deepEqual(after.child(1).attrs, fixture.state.doc.child(0).child(1).attrs);
    assert.equal(after.child(2).textContent, " 조사");
  } finally {
    fixture.close();
  }
});

await test("paragraph split and new heading allocate no IDs during proposal, preserving the mapped existing ID", () => {
  const fixture = setup(document(paragraph("p", "한글 연구")));
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const before = Y.encodeStateAsUpdate(fixture.ydoc);
    const proposal = fixture.session.prepare(capture, "한글\n\n# 연구", fixture.state);
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.ok(proposal.transaction);
    const after = fixture.state.apply(proposal.transaction).doc;
    assert.equal(after.child(0).attrs.id, "p");
    assert.equal(after.child(0).textContent, "한글");
    assert.equal(after.child(1).type.name, "heading");
    assert.equal(after.child(1).textContent, "연구");
    assert.equal(after.child(1).attrs.id, null);
    assert.deepEqual(Y.encodeStateAsUpdate(fixture.ydoc), before);
  } finally {
    fixture.close();
  }
});

await test("source code/math/callout edits retain identity and hidden code highlight metadata", () => {
  for (const [node, before, after, field] of [
    [
      {
        type: "codeBlock",
        attrs: { id: "code", language: "typescript", highlightLines: [2] },
        content: [{ type: "text", text: "const n = 1;" }],
      },
      "1",
      "2",
      "text",
    ],
    [{ type: "math", attrs: { id: "math", latex: "x + y" } }, "x", "z", "latex"],
    [
      { type: "mermaid", attrs: { id: "diagram", source: "graph TD; A-->B" } },
      "A-->",
      "C-->",
      "source",
    ],
    [
      {
        type: "callout",
        attrs: { id: "callout", kind: "note" },
        content: [paragraph("inside", "자료")],
      },
      "자료",
      "노트",
      "text",
    ],
  ] as const) {
    const fixture = setup(document(node));
    try {
      const capture = fixture.session.capture(fixture.state.doc);
      const proposal = fixture.session.prepare(
        capture,
        capture.source.replace(before, after),
        fixture.state,
      );
      assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
      assert.ok(proposal.transaction);
      const result = fixture.state.apply(proposal.transaction).doc.child(0);
      assert.equal(result.attrs.id, node.attrs.id);
      if (field === "text")
        assert.notEqual(result.textContent, fixture.state.doc.child(0).textContent);
      else assert.notEqual(result.attrs[field], fixture.state.doc.child(0).attrs[field]);
      if (node.type === "codeBlock") assert.deepEqual(result.attrs.highlightLines, [2]);
      if (node.type === "callout") assert.equal(result.child(0).attrs.id, "inside");
    } finally {
      fixture.close();
    }
  }
});

await test("unknown HTML and footnotes cannot silently disappear through the actual parser", () => {
  const fixture = setup(document(paragraph("p", "원본")));
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const before = Y.encodeStateAsUpdate(fixture.ydoc);
    for (const source of [
      "원본<script>bad</script>",
      "원본\n\n<div>unknown</div>",
      "원본[^x]\n\n[^x]: lost footnote",
    ]) {
      const proposal = fixture.session.prepare(capture, source, fixture.state);
      assert.equal(proposal.status, "loss");
      assert.ok(proposal.diagnostics[0]?.path.startsWith("source."));
      assert.deepEqual(Y.encodeStateAsUpdate(fixture.ydoc), before);
    }
  } finally {
    fixture.close();
  }
});

await test("unambiguous paragraph split keeps hidden underline/color and alignment on both resulting blocks", () => {
  const fixture = setup(
    document({
      type: "paragraph",
      attrs: { id: "split", textAlign: "right" },
      content: [
        {
          type: "text",
          text: "alpha beta",
          marks: [{ type: "underline" }, { type: "textStyle", attrs: { color: "#112233" } }],
        },
      ],
    }),
  );
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const proposal = fixture.session.prepare(capture, "alpha\n\nbeta", fixture.state);
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.ok(proposal.transaction);
    const after = fixture.state.apply(proposal.transaction).doc;
    assert.equal(after.childCount, 2);
    assert.equal(after.child(0).attrs.id, "split");
    assert.equal(after.child(1).attrs.id, null);
    for (let i = 0; i < 2; i++) {
      assert.equal(after.child(i).attrs.textAlign, "right");
      assert.deepEqual(after.child(i).child(0).marks, fixture.state.doc.child(0).child(0).marks);
    }
    assert.equal(after.child(0).textContent, "alpha");
    assert.equal(after.child(1).textContent, "beta");
  } finally {
    fixture.close();
  }
});

await test("replacement crossing distinct hidden mark runs warns with stable ID and Cancel leaves original raw bytes", () => {
  const fixture = setup(
    document(paragraph("untouched", "other"), {
      type: "paragraph",
      attrs: { id: "mixed" },
      content: [
        { type: "text", text: "alpha", marks: [{ type: "underline" }] },
        {
          type: "text",
          text: " beta",
          marks: [{ type: "textStyle", attrs: { color: "#112233" } }],
        },
      ],
    }),
  );
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const before = Y.encodeStateAsUpdate(fixture.ydoc);
    const proposal = fixture.session.prepare(
      capture,
      capture.source.replace("alpha beta", "ALPHA BETA"),
      fixture.state,
    );
    assert.equal(proposal.status, "loss");
    assert.equal(proposal.transaction, undefined);
    const diagnostic = proposal.diagnostics[0];
    assert.ok(diagnostic);
    assert.equal(diagnostic.id, "mixed");
    assert.equal(diagnostic.path, "document.content.1");
    assert.equal(diagnostic.field, "marks");
    assert.ok(diagnostic.reason.includes("underline"));
    assert.ok(diagnostic.reason.includes("textStyle"));
    assert.deepEqual(Y.encodeStateAsUpdate(fixture.ydoc), before);
  } finally {
    fixture.close();
  }
});

await test("a uniquely located split carries each different hidden mark only on its own surviving text", () => {
  const fixture = setup(
    document({
      type: "paragraph",
      attrs: { id: "split-runs", textAlign: "center" },
      content: [
        { type: "text", text: "alpha", marks: [{ type: "underline" }] },
        {
          type: "text",
          text: " beta",
          marks: [{ type: "textStyle", attrs: { color: "#112233" } }],
        },
      ],
    }),
  );
  try {
    const capture = fixture.session.capture(fixture.state.doc);
    const proposal = fixture.session.prepare(capture, "alpha\n\n# beta", fixture.state);
    assert.equal(proposal.status, "ready", JSON.stringify(proposal.diagnostics));
    assert.ok(proposal.transaction);
    const after = fixture.state.apply(proposal.transaction).doc;
    assert.equal(after.child(0).child(0).marks[0]?.type.name, "underline");
    assert.equal(after.child(1).child(0).marks[0]?.type.name, "textStyle");
    assert.equal(after.child(1).child(0).marks[0]?.attrs.color, "#112233");
    assert.equal(after.child(1).attrs.textAlign, "center");
    assert.equal(after.child(1).type.name, "heading");
  } finally {
    fixture.close();
  }
});
