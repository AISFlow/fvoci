import type { TiptapDoc } from "../src/json.js";

// A document, not a schema: unit and production-browser regressions both use
// createFvociExtensions and the real Rust seed/restore/export consumers.
export function schemaCorpus(refs: {
  user: string;
  document: string;
  attachment: string;
}): TiptapDoc {
  return {
    type: "doc",
    content: [
      {
        type: "heading",
        attrs: { id: "corpus-heading", level: 2 },
        content: [{ type: "text", text: "한국어 문서 😀" }],
      },
      {
        type: "paragraph",
        attrs: { id: "corpus-intro", textAlign: "center" },
        content: [
          { type: "text", text: "한글 보존 🧑‍💻 ", marks: [{ type: "bold" }, { type: "italic" }] },
          { type: "mention", attrs: { entity: "user", id: refs.user, label: "김철수" } },
          { type: "text", text: " 참조 " },
          { type: "mention", attrs: { entity: "document", id: refs.document, label: "관련 문서" } },
          { type: "text", text: " 수식 " },
          { type: "mathInline", attrs: { latex: "\\alpha + x^2" } },
          {
            type: "text",
            text: " 링크",
            marks: [
              { type: "link", attrs: { href: "https://example.com/한글", title: "한국어 링크" } },
              { type: "highlight", attrs: { color: "#ffe066" } },
            ],
          },
        ],
      },
      { type: "math", attrs: { id: "corpus-formula", latex: "\\int_0^1 x^2 dx = \\frac{1}{3}" } },
      {
        type: "table",
        attrs: { id: "corpus-table" },
        content: [
          {
            type: "tableRow",
            content: [
              {
                type: "tableHeader",
                attrs: { colspan: 1, rowspan: 1, colwidth: [180] },
                content: [
                  {
                    type: "paragraph",
                    attrs: { id: "corpus-header" },
                    content: [{ type: "text", text: "표 머리", marks: [{ type: "bold" }] }],
                  },
                ],
              },
              {
                type: "tableCell",
                attrs: { background: "#eeeeee", colspan: 1, rowspan: 1, colwidth: [220] },
                content: [
                  {
                    type: "paragraph",
                    attrs: { id: "corpus-cell" },
                    content: [{ type: "text", text: "중첩 셀" }],
                  },
                  {
                    type: "table",
                    attrs: { id: "corpus-inner-table" },
                    content: [
                      {
                        type: "tableRow",
                        content: [
                          {
                            type: "tableCell",
                            content: [
                              {
                                type: "paragraph",
                                attrs: { id: "corpus-inner-cell" },
                                content: [
                                  {
                                    type: "text",
                                    text: "안쪽 한글",
                                    marks: [
                                      { type: "underline" },
                                      { type: "textStyle", attrs: { color: "#112233" } },
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
              },
            ],
          },
        ],
      },
      {
        type: "bulletList",
        content: [
          {
            type: "listItem",
            attrs: { id: "corpus-item" },
            content: [
              {
                type: "paragraph",
                attrs: { id: "corpus-list-text" },
                content: [{ type: "text", text: "목록 항목" }],
              },
              {
                type: "orderedList",
                attrs: { start: 3 },
                content: [
                  {
                    type: "listItem",
                    attrs: { id: "corpus-nested-item" },
                    content: [
                      {
                        type: "paragraph",
                        attrs: { id: "corpus-nested-text" },
                        content: [{ type: "text", text: "세 번째", marks: [{ type: "strike" }] }],
                      },
                    ],
                  },
                ],
              },
            ],
          },
        ],
      },
      {
        type: "attachment",
        attrs: { id: refs.attachment, name: "한글 첨부.txt", image: false, caption: "검증 파일" },
      },
      {
        type: "paragraph",
        attrs: { id: "corpus-edit-tail" },
        content: [{ type: "text", text: "재편집 위치" }],
      },
    ],
  };
}
