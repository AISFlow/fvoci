import type { TiptapDoc } from "../src/json.ts";

// Handwritten semantic inputs and facts. Neither Markdown converter supplies
// an expected value. Losses describe the baseline projection, not W3 success.
export type CorpusValue =
  null | boolean | number | string | CorpusValue[] | { [key: string]: CorpusValue };

export interface CorpusNode {
  type: string;
  attrs?: Record<string, CorpusValue>;
  text?: string;
  marks?: { type: string; attrs?: Record<string, unknown> }[];
  content?: CorpusNode[];
}

export interface SemanticFact {
  path: string;
  value: unknown;
}

export interface ProjectionLoss {
  path: string;
  field: string;
  before: unknown;
  projected: "omitted" | "flattened" | "normalized" | "ambiguous";
}

export interface ContractCase {
  id: string;
  name: string;
  storage: "schema" | "raw";
  input: TiptapDoc;
  expected: SemanticFact[];
  losses: ProjectionLoss[];
  required: string;
  invalidInput?: TiptapDoc;
  sourceExamples?: string[];
}

export const corpusRefs = {
  user: "10000000-0000-4000-8000-000000000001",
  group: "10000000-0000-4000-8000-000000000002",
  document: "10000000-0000-4000-8000-000000000003",
  task: "10000000-0000-4000-8000-000000000004",
  project: "10000000-0000-4000-8000-000000000005",
  attachment: "10000000-0000-4000-8000-000000000006",
  image: "10000000-0000-4000-8000-000000000007",
} as const;

function text(value: string, marks?: CorpusNode["marks"]): CorpusNode {
  return { type: "text", text: value, ...(marks ? { marks } : {}) };
}

function paragraph(id: string, ...content: CorpusNode[]): CorpusNode {
  return { type: "paragraph", attrs: { id }, content };
}

function doc(...content: CorpusNode[]): TiptapDoc {
  return { type: "doc", content };
}

export const v050ContractCorpus: ContractCase[] = [
  {
    id: "F01",
    name: "Unicode, marks, hard break, empty block and TOC identity",
    storage: "schema",
    input: doc(
      {
        type: "heading",
        attrs: { id: "f01-heading", level: 2, "data-toc-id": "distinct-anchor" },
        content: [text("한글 🧑‍💻 ❤️")],
      },
      paragraph(
        "f01-body",
        text("연구", [{ type: "bold" }, { type: "italic" }]),
        { type: "hardBreak" },
        text(" ` x ` ", [{ type: "code" }]),
        text("취소", [{ type: "strike" }]),
        text("강조", [{ type: "highlight", attrs: { color: "#ffe066" } }]),
        text("링크", [{ type: "link", attrs: { href: "https://example.com/한글" } }]),
      ),
      paragraph("f01-empty"),
    ),
    expected: [
      { path: "content.0.attrs.id", value: "f01-heading" },
      { path: "content.0.attrs.data-toc-id", value: "distinct-anchor" },
      { path: "content.0.attrs.level", value: 2 },
      { path: "content.0.content.0.text", value: "한글 🧑‍💻 ❤️" },
      {
        path: "content.1.content.0.marks",
        value: [
          { type: "bold", attrs: {} },
          { type: "italic", attrs: {} },
        ],
      },
      { path: "content.1.content.1.type", value: "hardBreak" },
      { path: "content.1.content.2.text", value: " ` x ` " },
      { path: "content.1.content.2.marks.0.type", value: "code" },
      { path: "content.1.content.3.marks.0.type", value: "strike" },
      { path: "content.1.content.4.marks.0.attrs.color", value: "#ffe066" },
      { path: "content.1.content.5.marks.0.attrs.href", value: "https://example.com/한글" },
      { path: "content.2.attrs.id", value: "f01-empty" },
      { path: "content.2.content", value: undefined },
    ],
    losses: [
      { path: "content.0", field: "id", before: "f01-heading", projected: "omitted" },
      { path: "content.0", field: "data-toc-id", before: "distinct-anchor", projected: "omitted" },
      {
        path: "content.2",
        field: "empty paragraph identity",
        before: "f01-empty",
        projected: "omitted",
      },
    ],
    required:
      "No-op four-mode viewing preserves all IDs, exact Unicode and empty blocks without an update or undo item.",
  },
  {
    id: "F02",
    name: "Formatting attributes and whitespace marks",
    storage: "schema",
    input: doc({
      type: "paragraph",
      attrs: { id: "f02", textAlign: "right" },
      content: [
        text("색상", [{ type: "underline" }, { type: "textStyle", attrs: { color: "#112233" } }]),
        text("강조", [{ type: "highlight", attrs: { color: "#abcdef" } }]),
        text("링크", [
          {
            type: "link",
            attrs: {
              href: "https://example.com",
              target: "_self",
              rel: "author",
              class: "research",
              title: "자료",
            },
          },
        ]),
        text("   ", [{ type: "bold" }, { type: "italic" }]),
      ],
    }),
    expected: [
      { path: "content.0.attrs.textAlign", value: "right" },
      {
        path: "content.0.content.0.marks",
        value: [
          { type: "textStyle", attrs: { color: "#112233" } },
          { type: "underline", attrs: {} },
        ],
      },
      { path: "content.0.content.1.marks.0.attrs.color", value: "#abcdef" },
      {
        path: "content.0.content.2.marks.0.attrs",
        value: {
          href: "https://example.com",
          target: "_self",
          rel: "author",
          class: "research",
          title: "자료",
        },
      },
      { path: "content.0.content.3.text", value: "   " },
      {
        path: "content.0.content.3.marks",
        value: [
          { type: "bold", attrs: {} },
          { type: "italic", attrs: {} },
        ],
      },
    ],
    losses: [
      { path: "content.0", field: "textAlign", before: "right", projected: "omitted" },
      {
        path: "content.0.content.0",
        field: "underline/textStyle",
        before: "#112233",
        projected: "omitted",
      },
      {
        path: "content.0.content.1",
        field: "highlight.color",
        before: "#abcdef",
        projected: "omitted",
      },
      {
        path: "content.0.content.2",
        field: "link target/rel/class/title",
        before: ["_self", "author", "research", "자료"],
        projected: "omitted",
      },
      {
        path: "content.0.content.3",
        field: "whitespace emphasis",
        before: "bold+italic",
        projected: "omitted",
      },
    ],
    required:
      "Preserve untouched formatting; diagnose each touched unrepresentable field before Apply, with Cancel.",
  },
  {
    id: "F03",
    name: "Nested lists, checked state and containers",
    storage: "schema",
    input: doc(
      {
        type: "bulletList",
        content: [
          {
            type: "listItem",
            attrs: { id: "f03-item" },
            content: [
              paragraph("f03-p", text("항목")),
              {
                type: "orderedList",
                attrs: { start: 3, type: "a" },
                content: [
                  {
                    type: "listItem",
                    attrs: { id: "f03-nested" },
                    content: [paragraph("f03-nested-p", text("세 번째"))],
                  },
                ],
              },
            ],
          },
        ],
      },
      {
        type: "taskList",
        content: [true, false].map((checked, i) => ({
          type: "taskItem",
          attrs: { id: `f03-task-${String(i)}`, checked },
          content: [paragraph(`f03-check-${String(i)}`, text(checked ? "완료" : "대기"))],
        })),
      },
      {
        type: "blockquote",
        attrs: { id: "f03-quote" },
        content: [
          {
            type: "callout",
            attrs: { id: "f03-callout", kind: "warning" },
            content: [
              {
                type: "codeBlock",
                attrs: { id: "f03-code", language: "text" },
                content: [text("인용 코드")],
              },
              { type: "math", attrs: { id: "f03-math", latex: "x^2" } },
            ],
          },
        ],
      },
    ),
    expected: [
      { path: "content.0.content.0.attrs.id", value: "f03-item" },
      { path: "content.0.content.0.content.1.attrs.start", value: 3 },
      { path: "content.0.content.0.content.1.attrs.type", value: "a" },
      { path: "content.0.content.0.content.1.content.0.attrs.id", value: "f03-nested" },
      { path: "content.1.content.0.attrs.checked", value: true },
      { path: "content.1.content.1.attrs.checked", value: false },
      { path: "content.2.content.0.attrs.kind", value: "warning" },
      { path: "content.2.content.0.content.1.attrs.latex", value: "x^2" },
    ],
    losses: [
      {
        path: "content.0.content.0.content.1",
        field: "start/type",
        before: [3, "a"],
        projected: "normalized",
      },
      {
        path: "content.0.content.0",
        field: "nested block IDs",
        before: ["f03-item", "f03-nested"],
        projected: "omitted",
      },
    ],
    required:
      "Compare ordered nesting, child identities and true/false checks; task status is a separate object contract.",
  },
  {
    id: "F04",
    name: "Mixed table cells, geometry and nested table",
    storage: "raw",
    input: doc({
      type: "table",
      attrs: { id: "f04-table" },
      content: [
        {
          type: "tableRow",
          content: [
            {
              type: "tableHeader",
              attrs: {
                colspan: 2,
                rowspan: 2,
                colwidth: [180, 220],
                background: "#eeeeee",
                align: "right",
              },
              content: [
                paragraph("f04-header", text("표 머리")),
                paragraph("f04-second", text("둘째 문단")),
              ],
            },
            {
              type: "tableCell",
              content: [
                {
                  type: "table",
                  attrs: { id: "f04-inner" },
                  content: [
                    {
                      type: "tableRow",
                      content: [
                        {
                          type: "tableCell",
                          content: [
                            paragraph(
                              "f04-inner-p",
                              text("안쪽 한글", [{ type: "underline", attrs: {} }]),
                            ),
                          ],
                        },
                      ],
                    },
                  ],
                },
              ],
            },
          ],
        },
      ],
    }),
    expected: [
      { path: "content.0.attrs.id", value: "f04-table" },
      { path: "content.0.content.0.content.0.type", value: "tableHeader" },
      { path: "content.0.content.0.content.1.type", value: "tableCell" },
      {
        path: "content.0.content.0.content.0.attrs",
        value: {
          colspan: 2,
          rowspan: 2,
          colwidth: [180, 220],
          background: "#eeeeee",
          align: "right",
        },
      },
      { path: "content.0.content.0.content.0.content.1.attrs.id", value: "f04-second" },
      { path: "content.0.content.0.content.1.content.0.attrs.id", value: "f04-inner" },
      {
        path: "content.0.content.0.content.1.content.0.content.0.content.0.content.0.content.0.text",
        value: "안쪽 한글",
      },
    ],
    losses: [
      {
        path: "content.0",
        field: "nested cell structure and cell kinds",
        before: ["tableHeader", "tableCell", "table"],
        projected: "flattened",
      },
      {
        path: "content.0.content.0.content.0",
        field: "geometry/background/align",
        before: [2, 2, [180, 220], "#eeeeee", "right"],
        projected: "omitted",
      },
    ],
    required:
      "Raw align is already-stored extension data (not a supported schema attr); editing another paragraph leaves this table intact.",
  },
  {
    id: "F05",
    name: "Code, mermaid, exact math and atomic blocks",
    storage: "schema",
    input: doc(
      {
        type: "codeBlock",
        attrs: { id: "f05-code", language: "typescript", highlightLines: [1, 3] },
        content: [text("const x = `한글`;\n```\n끝")],
      },
      { type: "mermaid", attrs: { id: "f05-mermaid", source: "graph TD\nA-->B" } },
      { type: "math", attrs: { id: "f05-math", latex: "x + $$ + y" } },
      paragraph("f05-inline", { type: "mathInline", attrs: { latex: "x  + \\text{$5}" } }),
      {
        type: "callout",
        attrs: { id: "f05-callout", kind: "tip" },
        content: [paragraph("f05-tip", text("힌트"))],
      },
      { type: "horizontalRule", attrs: { id: "f05-rule" } },
    ),
    expected: [
      { path: "content.0.attrs.language", value: "typescript" },
      { path: "content.0.attrs.highlightLines", value: [1, 3] },
      { path: "content.0.content.0.text", value: "const x = `한글`;\n```\n끝" },
      { path: "content.1.attrs.source", value: "graph TD\nA-->B" },
      { path: "content.2.attrs.latex", value: "x + $$ + y" },
      { path: "content.3.content.0.attrs.latex", value: "x  + \\text{$5}" },
      { path: "content.4.attrs.kind", value: "tip" },
      { path: "content.5.attrs.id", value: "f05-rule" },
    ],
    losses: [
      { path: "content.0", field: "highlightLines", before: [1, 3], projected: "omitted" },
      {
        path: "content.2",
        field: "delimiter-containing latex",
        before: "x + $$ + y",
        projected: "ambiguous",
      },
      {
        path: "content.3.content.0",
        field: "latex whitespace",
        before: "x  + \\text{$5}",
        projected: "normalized",
      },
    ],
    required:
      "Keep exact latex and fence text; a shape-only self-check cannot certify attrs or identity preservation.",
  },
  {
    id: "F06",
    name: "Open details and marked atomic summary",
    storage: "raw",
    input: doc({
      type: "details",
      attrs: { id: "f06", open: true },
      content: [
        {
          type: "detailsSummary",
          attrs: { id: "f06-summary" },
          content: [
            text("요약", [{ type: "bold", attrs: {} }]),
            { type: "mathInline", attrs: { latex: "x^2" } },
          ],
        },
        {
          type: "detailsContent",
          attrs: { id: "f06-content" },
          content: [
            {
              type: "blockquote",
              attrs: { id: "f06-quote" },
              content: [paragraph("f06-p", text("내용"))],
            },
          ],
        },
      ],
    }),
    expected: [
      { path: "content.0.attrs.open", value: true },
      { path: "content.0.content.0.attrs.id", value: "f06-summary" },
      { path: "content.0.content.0.content.0.marks", value: [{ type: "bold", attrs: {} }] },
      { path: "content.0.content.0.content.1.attrs.latex", value: "x^2" },
      { path: "content.0.content.1.content.0.attrs.id", value: "f06-quote" },
    ],
    losses: [
      { path: "content.0", field: "open", before: true, projected: "omitted" },
      {
        path: "content.0.content.0",
        field: "summary marks and math atom",
        before: ["bold", "mathInline"],
        projected: "flattened",
      },
    ],
    required:
      "Current detailsSummary permits text only: this already-stored atom is raw/unsupported. Preserve it on Cancel; plain text or closed state is a loss.",
  },
  {
    id: "F07",
    name: "Reference tuples and multiplicity",
    storage: "schema",
    input: doc(
      paragraph(
        "f07-p",
        ...(["user", "group", "document", "task", "project", "task"] as const).map((entity) => ({
          type: "mention",
          attrs: { entity, id: corpusRefs[entity], label: "같은 이름" },
        })),
      ),
      ...(["document", "task", "project"] as const).map((entity) => ({
        type: "embed",
        attrs: { id: `f07-${entity}`, entity, ref: corpusRefs[entity] },
      })),
      { type: "embed", attrs: { id: "f07-url", entity: "url", ref: "https://example.com/source" } },
    ),
    expected: [
      {
        path: "content.0.content.0.attrs",
        value: { entity: "user", id: "10000000-0000-4000-8000-000000000001", label: "같은 이름" },
      },
      { path: "content.0.content.1.attrs.entity", value: "group" },
      { path: "content.0.content.2.attrs.id", value: "10000000-0000-4000-8000-000000000003" },
      { path: "content.0.content.3.attrs.id", value: "10000000-0000-4000-8000-000000000004" },
      { path: "content.0.content.4.attrs.entity", value: "project" },
      { path: "content.0.content.5.attrs.id", value: "10000000-0000-4000-8000-000000000004" },
      { path: "content.1.attrs.id", value: "f07-document" },
      { path: "content.2.attrs.ref", value: "10000000-0000-4000-8000-000000000004" },
      { path: "content.3.attrs.ref", value: "10000000-0000-4000-8000-000000000005" },
      {
        path: "content.4.attrs",
        value: { id: "f07-url", entity: "url", ref: "https://example.com/source" },
      },
    ],
    losses: [
      {
        path: "content.0.content.3",
        field: "mention entity/id",
        before: ["task", "10000000-0000-4000-8000-000000000004"],
        projected: "omitted",
      },
      { path: "content.1", field: "embed block ID", before: "f07-document", projected: "omitted" },
      { path: "content.4", field: "URL embed node kind", before: "embed", projected: "flattened" },
    ],
    required:
      "Check all ordered occurrences and the separate document/task backlink set; rename never retargets a reference.",
  },
  {
    id: "F08",
    name: "File identity and image layout",
    storage: "schema",
    input: doc(
      {
        type: "attachment",
        attrs: {
          id: corpusRefs.attachment,
          name: "한글 [자료](1).txt",
          image: false,
          width: 320,
          align: "left",
          caption: "읽기 자료",
          previewWidth: 640,
          previewHeight: 480,
        },
      },
      {
        type: "attachment",
        attrs: {
          id: corpusRefs.image,
          name: "그림.png",
          image: true,
          width: 320,
          align: "right",
          caption: "그림 설명",
          previewWidth: 640,
          previewHeight: 480,
        },
      },
    ),
    expected: [
      { path: "content.0.attrs.id", value: "10000000-0000-4000-8000-000000000006" },
      { path: "content.0.attrs.name", value: "한글 [자료](1).txt" },
      { path: "content.0.attrs.image", value: false },
      { path: "content.0.attrs.width", value: 320 },
      { path: "content.0.attrs.align", value: "left" },
      { path: "content.0.attrs.caption", value: "읽기 자료" },
      { path: "content.0.attrs.previewWidth", value: 640 },
      { path: "content.0.attrs.previewHeight", value: 480 },
      { path: "content.1.attrs.id", value: "10000000-0000-4000-8000-000000000007" },
      { path: "content.1.attrs.image", value: true },
      {
        path: "content.1.attrs",
        value: {
          id: "10000000-0000-4000-8000-000000000007",
          name: "그림.png",
          image: true,
          width: 320,
          align: "right",
          caption: "그림 설명",
          previewWidth: 640,
          previewHeight: 480,
        },
      },
    ],
    losses: [
      {
        path: "content.0",
        field: "width/align/caption/preview dimensions",
        before: [320, "left", "읽기 자료", 640, 480],
        projected: "omitted",
      },
      {
        path: "content.0",
        field: "punctuation-bearing name",
        before: "한글 [자료](1).txt",
        projected: "ambiguous",
      },
    ],
    required:
      "Unrelated edits retain every attachment tuple; projection cannot create uploads or allocate a new file ID.",
  },
  {
    id: "F09",
    name: "Glyph and custom emoji policy",
    storage: "schema",
    input: doc(
      paragraph(
        "f09",
        { type: "emoji", attrs: { name: "grinning" } },
        text("🧑‍💻", [{ type: "bold" }]),
        { type: "emoji", attrs: { name: "custom-no-glyph" } },
      ),
    ),
    expected: [
      { path: "content.0.content.0.attrs.name", value: "grinning" },
      { path: "content.0.content.1.text", value: "🧑‍💻" },
      { path: "content.0.content.1.marks", value: [{ type: "bold", attrs: {} }] },
      { path: "content.0.content.2.attrs.name", value: "custom-no-glyph" },
    ],
    invalidInput: doc(
      paragraph("invalid-emoji", {
        type: "emoji",
        attrs: { name: "custom-no-glyph" },
        marks: [{ type: "bold" }],
      }),
    ),
    losses: [
      {
        path: "content.0.content.2",
        field: "custom emoji without Unicode glyph",
        before: "custom-no-glyph",
        projected: "omitted",
      },
    ],
    required:
      "Only unmarked known glyph/text equivalence is allowed; marked atoms reject before mutation, custom glyph absence is never empty text equivalence.",
  },
  {
    id: "F10",
    name: "Already-stored future data",
    storage: "raw",
    input: doc(
      {
        type: "futureNode",
        attrs: { id: "f10-future", futureAttr: "미래 참조" },
        content: [text("한글 미래 😀", [{ type: "futureMark", attrs: { ref: "mark-ref" } }])],
      },
      {
        type: "paragraph",
        attrs: { id: "f10-known", futureAttr: "known-node-extension" },
        content: [text("기존 노드")],
      },
    ),
    expected: [
      { path: "content.0.type", value: "futureNode" },
      { path: "content.0.attrs", value: { id: "f10-future", futureAttr: "미래 참조" } },
      { path: "content.0.content.0.text", value: "한글 미래 😀" },
      {
        path: "content.0.content.0.marks",
        value: [{ type: "futureMark", attrs: { ref: "mark-ref" } }],
      },
      { path: "content.1.attrs.futureAttr", value: "known-node-extension" },
    ],
    losses: [
      {
        path: "content.0",
        field: "future node/mark/attr",
        before: ["futureNode", "futureMark", "futureAttr"],
        projected: "omitted",
      },
    ],
    required:
      "Observe raw content before schema coercion; refuse unsupported Apply and preserve it on Cancel.",
  },
  {
    id: "F11",
    name: "Literal source ambiguity and unsupported syntax",
    storage: "schema",
    input: doc(
      paragraph(
        "f11",
        text("[[task:literal]] @[user/not-an-id] | < $5-$10 \\"),
        text("!", [{ type: "bold" }]),
        text("[image](attachment:not-a-file)"),
      ),
    ),
    expected: [
      {
        path: "content.0.content.0.text",
        value: "[[task:literal]] @[user/not-an-id] | < $5-$10 \\",
      },
      { path: "content.0.content.1.text", value: "!" },
      { path: "content.0.content.1.marks", value: [{ type: "bold", attrs: {} }] },
      { path: "content.0.content.2.text", value: "[image](attachment:not-a-file)" },
    ],
    sourceExamples: [
      "[file](attachment:10000000-0000-4000-8000-000000000006)",
      "inline [file](attachment:10000000-0000-4000-8000-000000000006) tail",
      "<unknown>한글</unknown>",
      "[^note]\n\n[^note]: unsupported",
    ],
    losses: [
      {
        path: "content.0",
        field: "literal/reference and split image boundary",
        before: "literal text",
        projected: "ambiguous",
      },
    ],
    required:
      "Sole attachment paragraph and inline link differ; parse return alone cannot certify literal/reference or ignored HTML/footnotes.",
  },
  {
    id: "F12",
    name: "Missing, duplicate and domain-colliding IDs",
    storage: "schema",
    input: doc(
      { type: "paragraph", content: [text("동일 문단")] },
      paragraph("duplicate", text("동일 문단")),
      paragraph("duplicate", text("동일 문단")),
      paragraph(corpusRefs.attachment, text("별도 블록")),
      { type: "attachment", attrs: { id: corpusRefs.attachment, name: "file.txt", image: false } },
    ),
    expected: [
      { path: "content.0.attrs", value: undefined },
      { path: "content.1.attrs.id", value: "duplicate" },
      { path: "content.2.attrs.id", value: "duplicate" },
      { path: "content.3.attrs.id", value: "10000000-0000-4000-8000-000000000006" },
      { path: "content.4.attrs.id", value: "10000000-0000-4000-8000-000000000006" },
    ],
    losses: [
      {
        path: "content.1",
        field: "ambiguous block mapping",
        before: "duplicate",
        projected: "ambiguous",
      },
    ],
    required:
      "Schema validity does not establish unique identity. Refuse ambiguous Apply; viewing/Cancel allocates no IDs. File UUID and block ID are distinct domains.",
  },
];
