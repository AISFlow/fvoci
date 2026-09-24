#!/usr/bin/env node
/**
 * Dev-only generator: pinned Yjs 13.6.32 + public Tiptap extensions write
 * updateV1 / state-vector / snapshot bytes into ../fixtures.
 * Product code must never import this package or spawn Node.
 */
import { mkdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import { getSchema, Node } from "@tiptap/core";
import StarterKit from "@tiptap/starter-kit";
import { TableKit } from "@tiptap/extension-table/kit";
import { UniqueID } from "@tiptap/extension-unique-id";
import {
  prosemirrorJSONToYXmlFragment,
  yDocToProsemirrorJSON,
} from "@tiptap/y-tiptap";
import * as Y from "yjs";

const require = createRequire(import.meta.url);
const yjsVersion = require("yjs/package.json").version;
if (yjsVersion !== "13.6.32") {
  throw new Error(`expected yjs 13.6.32, got ${yjsVersion}`);
}

const FRAGMENT = "prosemirror";
const here = dirname(fileURLToPath(import.meta.url));
const outDir = join(here, "..", "fixtures");

const MentionLike = Node.create({
  name: "mention",
  group: "inline",
  inline: true,
  atom: true,
  addAttributes() {
    return {
      entity: { default: "user" },
      id: { default: "" },
      label: { default: "" },
    };
  },
  parseHTML() {
    return [{ tag: "span[data-mention]" }];
  },
  renderHTML({ HTMLAttributes }) {
    return ["span", { "data-mention": "", ...HTMLAttributes }, ""];
  },
});

const schema = getSchema([
  StarterKit,
  TableKit,
  UniqueID.configure({
    types: [
      "paragraph",
      "heading",
      "blockquote",
      "table",
      "tableRow",
      "tableHeader",
      "tableCell",
    ],
  }),
  MentionLike,
]);

function docWithClient(clientID) {
  const doc = new Y.Doc({ gc: false });
  doc.clientID = clientID;
  return doc;
}

function seedJson(json, clientID) {
  const doc = docWithClient(clientID);
  prosemirrorJSONToYXmlFragment(
    schema,
    json,
    doc.getXmlFragment(FRAGMENT),
  );
  return doc;
}

function insertParagraph(doc, text) {
  const frag = doc.getXmlFragment(FRAGMENT);
  const el = new Y.XmlElement("paragraph");
  el.setAttribute("id", `p-follow-${frag.length}`);
  const t = new Y.XmlText();
  t.insert(0, text);
  el.insert(0, [t]);
  frag.insert(frag.length, [el]);
}

function writeBin(name, bytes) {
  writeFileSync(join(outDir, name), Buffer.from(bytes));
}

const structuredJson = {
  type: "doc",
  content: [
    {
      type: "paragraph",
      attrs: { id: "p-alpha-001" },
      content: [
        { type: "text", text: "안녕 본문 " },
        {
          type: "text",
          marks: [
            {
              type: "link",
              attrs: { href: "https://example.invalid/wiki/안녕", target: "_blank" },
            },
          ],
          text: "링크",
        },
        { type: "text", text: " 🚀" },
        {
          type: "mention",
          attrs: { entity: "user", id: "user-42", label: "영희" },
        },
      ],
    },
    {
      type: "table",
      attrs: { id: "tbl-001" },
      content: [
        {
          type: "tableRow",
          attrs: { id: "tr-001" },
          content: [
            {
              type: "tableHeader",
              attrs: { id: "th-001" },
              content: [
                {
                  type: "paragraph",
                  attrs: { id: "th-p-001" },
                  content: [{ type: "text", text: "열" }],
                },
              ],
            },
            {
              type: "tableHeader",
              attrs: { id: "th-002" },
              content: [
                {
                  type: "paragraph",
                  attrs: { id: "th-p-002" },
                  content: [{ type: "text", text: "값" }],
                },
              ],
            },
          ],
        },
        {
          type: "tableRow",
          attrs: { id: "tr-002" },
          content: [
            {
              type: "tableCell",
              attrs: { id: "td-001" },
              content: [
                {
                  type: "paragraph",
                  attrs: { id: "td-p-001" },
                  content: [{ type: "text", text: "한글셀" }],
                },
              ],
            },
            {
              type: "tableCell",
              attrs: { id: "td-002" },
              content: [
                {
                  type: "paragraph",
                  attrs: { id: "td-p-002" },
                  content: [{ type: "text", text: "✨" }],
                },
              ],
            },
          ],
        },
      ],
    },
  ],
};

mkdirSync(outDir, { recursive: true });

const structured = seedJson(structuredJson, 11);
const structuredV1 = Y.encodeStateAsUpdate(structured);
writeBin("structured.v1", structuredV1);
const structuredPm = yDocToProsemirrorJSON(structured, FRAGMENT);

const ko = seedJson(
  {
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: "p-ko-001" },
        content: [{ type: "text", text: "가나다🚀마바사" }],
      },
    ],
  },
  21,
);
const koBase = Y.encodeStateAsUpdate(ko);
writeBin("korean_emoji_base.v1", koBase);
const frag = ko.getXmlFragment(FRAGMENT);
const para = frag.get(0);
const xmlText = para.get(0);
xmlText.insert(3, "중");
const koMid = Y.encodeStateAsUpdate(ko, Y.encodeStateVectorFromUpdate(koBase));
writeBin("korean_emoji_mid_edit.v1", koMid);
xmlText.delete(1, 2);
const koDel = Y.encodeStateAsUpdate(
  ko,
  Y.encodeStateVectorFromUpdate(Y.mergeUpdates([koBase, koMid])),
);
writeBin("korean_emoji_delete.v1", koDel);
const koAfterPm = yDocToProsemirrorJSON(ko, FRAGMENT);

const delSrc = seedJson(
  {
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: "p-del-001" },
        content: [{ type: "text", text: "유지한글🚀끝" }],
      },
    ],
  },
  31,
);
const delFull = Y.encodeStateAsUpdate(delSrc);
const svBefore = Y.encodeStateVector(delSrc);
const delText = delSrc.getXmlFragment(FRAGMENT).get(0).get(0);
delText.delete(2, 2);
const svAfter = Y.encodeStateVector(delSrc);
const deleteOnly = Y.encodeStateAsUpdate(delSrc, svBefore);
writeBin("delete_only_base.v1", delFull);
writeBin("delete_only.v1", deleteOnly);
writeBin("sv_before_delete.bin", svBefore);
writeBin("sv_after_delete.bin", svAfter);

const seq = seedJson(
  {
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: "p-seq-001" },
        content: [{ type: "text", text: "one" }],
      },
    ],
  },
  7,
);
const pendingU1 = Y.encodeStateAsUpdate(seq);
seq.transact(() => {
  insertParagraph(seq, "two한글");
});
const pendingU2 = Y.encodeStateAsUpdate(seq, Y.encodeStateVectorFromUpdate(pendingU1));
writeBin("pending_u1.v1", pendingU1);
writeBin("pending_u2.v1", pendingU2);

const rev = new Y.Doc({ gc: false });
rev.clientID = 41;
const arr = rev.getArray("t");
arr.insert(0, ["스냅샷-본문"]);
const ySnap = Y.snapshot(rev);
const snapBytes = Y.encodeSnapshot(ySnap);
const beforeRev = Y.encodeStateAsUpdate(rev);
rev.transact(() => {
  arr.delete(0, 1);
  arr.insert(0, ["compact-이후"]);
});
const afterRev = Y.encodeStateAsUpdate(rev);
writeBin("revision_before.v1", beforeRev);
writeBin("revision_after.v1", afterRev);
writeBin("revision_snapshot.bin", snapBytes);

const follow = new Y.Doc({ gc: false });
follow.clientID = 51;
Y.applyUpdate(follow, structuredV1);
follow.transact(() => {
  insertParagraph(follow, "후속편집한글✨");
});
const followUp = Y.encodeStateAsUpdate(follow, Y.encodeStateVector(structured));
writeBin("followup_edit.v1", followUp);
const followPm = yDocToProsemirrorJSON(follow, FRAGMENT);

const utf8Probe = seedJson(
  {
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: "p-utf8-001" },
        content: [{ type: "text", text: "안녕" }],
      },
    ],
  },
  61,
);
writeBin("utf8_korean.v1", Y.encodeStateAsUpdate(utf8Probe));

function b64(u8) {
  return Buffer.from(u8).toString("base64");
}

/** Source packages/editor/src/collab-tiptap.ts withoutYChange (3937952). */
function withoutYChange(value) {
  if (Array.isArray(value)) return value.map(withoutYChange);
  if (typeof value !== "object" || value === null) return value;
  const out = {};
  for (const [key, child] of Object.entries(value)) {
    if (key === "ychange") continue;
    if (key === "marks" && Array.isArray(child)) {
      out.marks = child.filter(
        (mark) => !(typeof mark === "object" && mark !== null && mark.type === "ychange"),
      );
      continue;
    }
    out[key] = withoutYChange(child);
  }
  return out;
}

function projectJson(doc) {
  return withoutYChange(yDocToProsemirrorJSON(doc, FRAGMENT));
}

const emptyPm = projectJson(new Y.Doc({ gc: false }));

const emptyPara = docWithClient(71);
{
  const frag = emptyPara.getXmlFragment(FRAGMENT);
  const p = new Y.XmlElement("paragraph");
  p.setAttribute("id", "p-empty-001");
  frag.insert(0, [p]);
}
writeBin("empty_paragraph.v1", Y.encodeStateAsUpdate(emptyPara));
const emptyParaPm = projectJson(emptyPara);

const typed = docWithClient(72);
{
  const frag = typed.getXmlFragment(FRAGMENT);
  const heading = new Y.XmlElement("heading");
  heading.setAttribute("id", "h-typed-001");
  heading.setAttribute("level", 2);
  heading.setAttribute("checked", false);
  heading.setAttribute("highlightLines", []);
  heading.setAttribute("nullable", null);
  heading.setAttribute("flag", true);
  const t = new Y.XmlText();
  t.insert(0, "typed한글");
  heading.insert(0, [t]);
  frag.insert(0, [heading]);
}
writeBin("typed_attrs.v1", Y.encodeStateAsUpdate(typed));
const typedPm = projectJson(typed);

const marksDoc = seedJson(
  {
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: "p-marks-001" },
        content: [
          { type: "text", marks: [{ type: "bold" }], text: "굵게" },
          { type: "text", text: " " },
          {
            type: "text",
            marks: [
              {
                type: "link",
                attrs: { href: "https://example.invalid/b", target: "_blank" },
              },
            ],
            text: "링크",
          },
        ],
      },
    ],
  },
  73,
);
writeBin("marks_link_bold.v1", Y.encodeStateAsUpdate(marksDoc));
const marksPm = projectJson(marksDoc);

const ychangeDoc = seedJson(
  {
    type: "doc",
    content: [
      {
        type: "paragraph",
        attrs: { id: "p-ychange-001" },
        content: [{ type: "text", text: "원문한글" }],
      },
    ],
  },
  74,
);
{
  const para = ychangeDoc.getXmlFragment(FRAGMENT).get(0);
  para.setAttribute("ychange", { type: "added" });
  const xmlText = para.get(0);
  xmlText.format(0, 2, { ychange: { type: "added" } });
}
writeBin("ychange_strip.v1", Y.encodeStateAsUpdate(ychangeDoc));
const ychangePmRaw = yDocToProsemirrorJSON(ychangeDoc, FRAGMENT);
const ychangePm = withoutYChange(ychangePmRaw);

const delBeforePm = projectJson(
  (() => {
    const d = new Y.Doc({ gc: false });
    Y.applyUpdate(d, delFull);
    return d;
  })(),
);
const delAfterPm = projectJson(
  (() => {
    const d = new Y.Doc({ gc: false });
    Y.applyUpdate(d, delFull);
    Y.applyUpdate(d, deleteOnly);
    return d;
  })(),
);

const pendingU1Pm = projectJson(
  (() => {
    const d = new Y.Doc({ gc: false });
    Y.applyUpdate(d, pendingU1);
    return d;
  })(),
);
const pendingU2OnlyPm = projectJson(
  (() => {
    const d = new Y.Doc({ gc: false });
    Y.applyUpdate(d, pendingU2);
    return d;
  })(),
);
const pendingBothPm = projectJson(
  (() => {
    const d = new Y.Doc({ gc: false });
    Y.applyUpdate(d, pendingU2);
    Y.applyUpdate(d, pendingU1);
    return d;
  })(),
);

const expectations = {
  yjs: "13.6.32",
  fragment: FRAGMENT,
  encoding: 1,
  gc: false,
  generator: "crates/collab-engine/js/generate.mjs",
  source_contract_sha: "393795261322b916e588043cf94feca999175843",
  structured: {
    must_include: ["안녕 본문", "링크", "🚀", "영희", "한글셀", "✨"],
    attrs: {
      paragraph_id: "p-alpha-001",
      table_id: "tbl-001",
      mention_id: "user-42",
      mention_entity: "user",
      href: "https://example.invalid/wiki/안녕",
    },
    prosemirror_type: structuredPm.type,
    prosemirror_json: projectJson(structured),
  },
  korean_emoji: {
    after_mid_and_delete_includes: ["가", "중", "🚀", "마바사"],
    after_mid_and_delete_excludes: ["나다"],
    prosemirror_text: JSON.stringify(koAfterPm),
    prosemirror_json: withoutYChange(koAfterPm),
  },
  delete_only: {
    state_vector_unchanged: Buffer.from(svBefore).equals(Buffer.from(svAfter)),
    sv_before_b64: b64(svBefore),
    sv_after_b64: b64(svAfter),
    prosemirror_json_before: delBeforePm,
    prosemirror_json_after: delAfterPm,
  },
  pending: {
    u1_has: "one",
    u2_has: "two한글",
    prosemirror_json_u1: pendingU1Pm,
    prosemirror_json_u2_only: pendingU2OnlyPm,
    prosemirror_json_both: pendingBothPm,
  },
  revision: {
    before: ["스냅샷-본문"],
    after: ["compact-이후"],
  },
  followup: {
    must_include: ["후속편집한글✨", "안녕 본문"],
    prosemirror_type: followPm.type,
    prosemirror_json: projectJson(follow),
  },
  utf8_marker: "안녕",
  empty_doc: {
    prosemirror_json: emptyPm,
  },
  empty_paragraph: {
    prosemirror_json: emptyParaPm,
  },
  typed_attrs: {
    prosemirror_json: typedPm,
  },
  marks_link_bold: {
    prosemirror_json: marksPm,
  },
  ychange_strip: {
    raw_has_ychange: JSON.stringify(ychangePmRaw).includes("ychange"),
    prosemirror_json: ychangePm,
  },
};

writeFileSync(
  join(outDir, "expectations.json"),
  `${JSON.stringify(expectations, null, 2)}\n`,
);

console.log(
  JSON.stringify(
    {
      ok: true,
      yjs: yjsVersion,
      files: [
        "structured.v1",
        "korean_emoji_base.v1",
        "korean_emoji_mid_edit.v1",
        "korean_emoji_delete.v1",
        "delete_only_base.v1",
        "delete_only.v1",
        "sv_before_delete.bin",
        "sv_after_delete.bin",
        "pending_u1.v1",
        "pending_u2.v1",
        "revision_before.v1",
        "revision_after.v1",
        "revision_snapshot.bin",
        "followup_edit.v1",
        "utf8_korean.v1",
        "empty_paragraph.v1",
        "typed_attrs.v1",
        "marks_link_bold.v1",
        "ychange_strip.v1",
        "expectations.json",
      ],
      delete_only_sv_unchanged: expectations.delete_only.state_vector_unchanged,
    },
    null,
    2,
  ),
);
