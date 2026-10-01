import { t } from "@fvoci/i18n";
import {
  type Editor,
  Extension,
  InputRule,
  Mark,
  mergeAttributes,
  textblockTypeInputRule,
} from "@tiptap/core";
import { CodeBlockLowlight } from "@tiptap/extension-code-block-lowlight";
import { isChangeOrigin } from "@tiptap/extension-collaboration";
import { Details, DetailsContent, DetailsSummary } from "@tiptap/extension-details";
import { Emoji, type EmojiItem, shortcodeToEmoji } from "@tiptap/extension-emoji";
import { Highlight } from "@tiptap/extension-highlight";
import { TaskItem, TaskList } from "@tiptap/extension-list";
import { NodeRange } from "@tiptap/extension-node-range";
import { TableKit } from "@tiptap/extension-table/kit";
import { TableOfContents } from "@tiptap/extension-table-of-contents";
import { TextAlign } from "@tiptap/extension-text-align";
import { Color, TextStyle } from "@tiptap/extension-text-style";
import { UniqueID } from "@tiptap/extension-unique-id";
import { Placeholder } from "@tiptap/extensions";
import { type EditorState, Plugin, PluginKey, Selection } from "@tiptap/pm/state";
import { StarterKit } from "@tiptap/starter-kit";
import type { SuggestionOptions } from "@tiptap/suggestion";
import { UNIQUE_ID_NODE_TYPES } from "./extract.js";
import {
  ensureLanguage,
  HIGHLIGHT_MAX_CHARS,
  languageOfFence,
  lowlight,
  parseFenceLanguage,
  parseHighlightLines,
} from "./lowlight.js";
import { Attachment } from "./nodes/attachment.js";
import { Callout } from "./nodes/callout.js";
import { Embed } from "./nodes/embed.js";
import { MathBlock, MathInline } from "./nodes/math.js";
import { Mention } from "./nodes/mention.js";
import { Mermaid } from "./nodes/mermaid.js";

export type EmojiMenuItem = EmojiItem & { title: string };

const attemptedLangs = new Set<string>();

// TOC 3.31.3 assigns both attrs when data-toc-id is absent, overwriting the
// block id used by body/blocks and internal links. Seed its separate anchor
// from an existing unique block id before the inherited lifecycle/plugin runs.
function preserveHeadingIds(state: EditorState, getId?: (text: string) => string) {
  const reservedAnchors = new Set<string>();
  state.doc.descendants((node) => {
    if (node.type.name === "heading" && typeof node.attrs["data-toc-id"] === "string") {
      reservedAnchors.add(node.attrs["data-toc-id"]);
    }
  });
  const blockIds = new Set<string>();
  const anchors = new Set<string>();
  const tr = state.tr;
  state.doc.descendants((node, pos) => {
    if (node.type.name !== "heading" || !node.textContent) return;
    const id: unknown = node.attrs.id;
    const anchor: unknown = node.attrs["data-toc-id"];
    if (typeof id !== "string" || !id) {
      if (typeof anchor === "string" && anchor && !blockIds.has(anchor) && !anchors.has(anchor)) {
        blockIds.add(anchor);
        anchors.add(anchor);
        tr.setNodeMarkup(pos, undefined, { ...node.attrs, id: anchor });
      }
      return; // Inherited TOC supplies IDs when both are missing.
    }
    if (blockIds.has(id)) {
      // Invalid duplicate identities cannot both name one block. Keep the first;
      // let the SDK allocate a fresh identity/anchor to the duplicate.
      tr.setNodeMarkup(pos, undefined, { ...node.attrs, id: null, "data-toc-id": null });
      return;
    }
    blockIds.add(id);
    if (typeof anchor === "string" && anchor && !anchors.has(anchor)) {
      anchors.add(anchor);
      return;
    }
    const next = reservedAnchors.has(id) ? (getId?.(node.textContent) ?? crypto.randomUUID()) : id;
    anchors.add(next);
    reservedAnchors.add(next);
    tr.setNodeMarkup(pos, undefined, { ...node.attrs, "data-toc-id": next });
  });
  return tr.steps.length ? tr : null;
}

const StableTableOfContents = TableOfContents.extend({
  onCreate(event) {
    if (typeof window !== "undefined") {
      const tr = preserveHeadingIds(this.editor.state, this.options.getId);
      if (tr) this.editor.view.dispatch(tr);
    }
    this.parent?.(event);
  },
  addProseMirrorPlugins() {
    return [
      new Plugin({
        key: new PluginKey("fvociHeadingIdentity"),
        appendTransaction: (transactions, _oldState, newState) => {
          if (
            typeof window === "undefined" ||
            !transactions.some((tr) => tr.docChanged) ||
            transactions.some((tr) => tr.getMeta("composition"))
          )
            return null;
          return preserveHeadingIds(newState, this.options.getId);
        },
      }),
      ...(this.parent?.() ?? []),
    ];
  },
});

const MarkedEmoji = Emoji.extend({
  addProseMirrorPlugins() {
    // Reuse the SDK's matching, composition guards, selection and suggestion
    // behavior. Marked Unicode must remain text: y-tiptap 3.0.9 stores marks
    // on XmlText, but does not encode marks on an XmlElement/emoji atom.
    const inherited = (this.parent?.() ?? []).map((plugin) => {
      const append = plugin.spec.appendTransaction;
      if (!append) return plugin;
      return new Plugin<unknown>({
        ...plugin.spec,
        appendTransaction(transactions, oldState, newState) {
          const tr = append.call(this, transactions, oldState, newState);
          if (!tr) return tr;
          const originalPositions = tr.mapping.invert();
          const marked: { pos: number; text: string; source: typeof newState.doc }[] = [];
          tr.doc.descendants((node, pos) => {
            if (node.type.name !== "emoji") return;
            const from = originalPositions.map(pos);
            const source = newState.doc.nodeAt(from);
            if (source?.isText && source.marks.length) {
              const to = originalPositions.map(pos + node.nodeSize, -1);
              marked.push({ pos, text: newState.doc.textBetween(from, to), source });
            }
          });
          for (const { pos, text, source } of marked.reverse()) {
            tr.replaceWith(pos, pos + 1, newState.schema.text(text, source.marks));
          }
          if (tr.doc.eq(newState.doc)) return null;
          // Replacement clears storedMarks; keep an explicit toolbar/input
          // choice instead of inheriting marks from the last converted emoji.
          if (newState.storedMarks) tr.setStoredMarks(newState.storedMarks);
          return tr;
        },
      });
    });
    return [
      new Plugin({
        key: new PluginKey("fvociEmojiMarkEncoding"),
        filterTransaction: (tr) => {
          let representable = true;
          tr.doc.descendants((node) => {
            if (node.type.name !== "emoji" || !node.marks.length) return;
            const name: unknown = node.attrs.name;
            if (typeof name !== "string" || !shortcodeToEmoji(name, this.options.emojis)?.emoji)
              representable = false;
          });
          return representable;
        },
        appendTransaction: (transactions, _oldState, state) => {
          if (!transactions.some((tr) => tr.docChanged) || this.editor.view.composing) return null;
          const marked: { pos: number; glyph: string; node: typeof state.doc }[] = [];
          state.doc.descendants((node, pos) => {
            if (node.type.name !== "emoji" || !node.marks.length) return;
            const name: unknown = node.attrs.name;
            const glyph =
              typeof name === "string"
                ? shortcodeToEmoji(name, this.options.emojis)?.emoji
                : undefined;
            if (glyph) marked.push({ pos, glyph, node });
          });
          if (!marked.length) return null;
          const tr = state.tr;
          for (const { pos, glyph, node } of marked.reverse()) {
            tr.replaceWith(pos, pos + 1, state.schema.text(glyph, node.marks));
          }
          if (state.storedMarks) tr.setStoredMarks(state.storedMarks);
          return tr;
        },
      }),
      ...inherited,
    ];
  },
});

const YCHANGE_NODE_TYPES = [
  "heading",
  "paragraph",
  "blockquote",
  "codeBlock",
  "embed",
  "listItem",
  "table",
  "horizontalRule",
  "bulletList",
  "orderedList",
  "tableRow",
  "tableHeader",
  "tableCell",
  "callout",
  "mermaid",
  "math",
  "attachment",
  "details",
  "detailsContent",
  "detailsSummary",
  "taskList",
  "taskItem",
] as const;

function ychangeTypeOf(value: unknown): "added" | "removed" | null {
  if (typeof value !== "object" || value === null) return null;
  const type: unknown = Reflect.get(value, "type");
  return type === "added" || type === "removed" ? type : null;
}

function dataAttr(el: unknown, name: string): string | null {
  if (typeof el !== "object" || el === null) return null;
  const get: unknown = Reflect.get(el, "getAttribute");
  if (typeof get !== "function") return null;
  const v: unknown = Reflect.apply(get, el, [name]);
  return typeof v === "string" ? v : null;
}

const YChangeAttr = Extension.create({
  name: "ychangeAttr",
  addGlobalAttributes() {
    return [
      {
        types: [...YCHANGE_NODE_TYPES],
        attributes: {
          ychange: {
            default: null,
            parseHTML: (el) => {
              const type = dataAttr(el, "data-ychange-type");
              return type === "added" || type === "removed" ? { type } : null;
            },
            renderHTML: (attributes) => {
              const type = ychangeTypeOf(attributes.ychange);
              return type === null ? {} : { "data-ychange-type": type };
            },
          },
        },
      },
    ];
  },
});

const YChangeMark = Mark.create({
  name: "ychange",
  addAttributes() {
    return {
      user: { default: null },
      type: {
        default: null,
        parseHTML: (el) => dataAttr(el, "data-ychange-type"),
        renderHTML: (attributes) => {
          const type: unknown = attributes.type;
          return type === "added" || type === "removed" ? { "data-ychange-type": type } : {};
        },
      },
      color: {
        default: null,
        renderHTML: () => ({}),
      },
    };
  },
  parseHTML() {
    return [{ tag: "span[data-ychange-type]" }];
  },
  renderHTML({ HTMLAttributes }) {
    return ["span", mergeAttributes(HTMLAttributes), 0];
  },
});

function emojiMenuItems({ editor, query }: { editor: Editor; query: string }): EmojiMenuItem[] {
  const q = query.trim().toLowerCase();
  return editor.storage.emoji.emojis
    .filter(
      (item) =>
        item.name.toLowerCase().startsWith(q) ||
        item.shortcodes.some((code) => code.toLowerCase().startsWith(q)),
    )
    .slice(0, 8)
    .map((item) => ({
      ...item,
      title: `${item.emoji ?? ""} ${item.name}`.trim(),
    }));
}

export function createFvociExtensions(opts?: {
  emojiSuggestionRender?: SuggestionOptions<EmojiMenuItem>["render"];
  emojiSuggestionFloatingUi?: SuggestionOptions<EmojiMenuItem>["floatingUi"];
}) {
  return [
    StarterKit.configure({
      undoRedo: false,
      codeBlock: false,
    }),
    CodeBlockLowlight.extend({
      addAttributes() {
        return {
          ...this.parent?.(),
          highlightLines: {
            default: [],
            parseHTML: (el) => parseHighlightLines(el.getAttribute("data-highlight-lines") ?? ""),
            renderHTML: (attributes) => {
              const lines: unknown = attributes.highlightLines;
              if (!Array.isArray(lines) || lines.length === 0) return {};
              return { "data-highlight-lines": lines.join(",") };
            },
          },
        };
      },
      addInputRules() {
        return [
          textblockTypeInputRule({
            find: /^```(?!mermaid)([a-z]+(?:\{[\d,\-\s]+\})?)?[\s\n]$/,
            type: this.type,
            getAttributes: (match) => {
              const parsed = parseFenceLanguage(match[1]);
              ensureLanguage(parsed.language).catch((error: unknown) => {
                console.error("Failed to load editor code-block language", parsed.language, error);
              });
              return parsed;
            },
          }),
        ];
      },
    }).configure({
      lowlight,
      enableTabIndentation: true,
      tabSize: 2,
    }),
    Extension.create({
      name: "fvociLowlightPack",
      onTransaction({ editor, transaction }) {
        /* WHY: #571 — 선택 이동·원격 awareness 트랜잭션까지 문서를 전수 순회했다.
         * 코드블록 언어는 문서가 바뀔 때만 새로 생긴다. */
        if (!transaction.docChanged) return;
        const seen = new Set<string>();
        editor.state.doc.descendants((node) => {
          if (node.type.name !== "codeBlock") return;
          if (node.textContent.length > HIGHLIGHT_MAX_CHARS) return;
          const language = String(node.attrs.language ?? "");
          if (language) seen.add(language);
        });
        for (const language of seen) {
          const key = languageOfFence(language) || language;
          if (lowlight.listLanguages().includes(key)) continue;
          if (attemptedLangs.has(key)) continue;
          attemptedLangs.add(key);
          ensureLanguage(key)
            .then((resolved) => {
              if (editor.isDestroyed) return;
              if (!lowlight.listLanguages().includes(resolved)) return;
              const { tr, doc, selection, storedMarks } = editor.state;
              doc.descendants((node, pos) => {
                if (node.type.name !== "codeBlock") return;
                // WHY: Lowlight needs a node-covering step; preserve its content mapping.
                tr.setNodeMarkup(pos, undefined, node.attrs);
              });
              if (tr.steps.length === 0) return;
              // WHY: Equivalent markup refresh replaces node boundaries in the step map.
              tr.setSelection(Selection.fromJSON(tr.doc, selection.toJSON()));
              tr.setStoredMarks(storedMarks);
              tr.setMeta("addToHistory", false);
              tr.setMeta("fvociLowlight", resolved);
              editor.view.dispatch(tr);
            })
            .catch((error: unknown) => {
              console.error("Failed to refresh editor code-block language", key, error);
            });
        }
      },
    }),
    TableKit.configure({ table: { resizable: true } }),
    Extension.create({
      name: "tableCellBackground",
      addGlobalAttributes() {
        return [
          {
            types: ["tableCell", "tableHeader"],
            attributes: {
              background: {
                default: null,
                parseHTML: (el) => dataAttr(el, "data-background"),
                renderHTML: (attributes) => {
                  const bg: unknown = attributes.background;
                  if (typeof bg !== "string" || bg.length === 0) return {};
                  return {
                    "data-background": bg,
                    style: `background:${bg}`,
                  };
                },
              },
            },
          },
        ];
      },
    }),
    NodeRange,
    UniqueID.configure({
      types: [...UNIQUE_ID_NODE_TYPES],
      /* WHY: #258 — a block without an id (a body the server seeded, a new
       * document's first paragraph) gets one on the first local edit in it.
       * UniqueID (@tiptap/extension-unique-id 3.31.3, the latest) appends
       * that setNodeMarkup to the same dispatch, so when the edit is an IME
       * composition's first update, ProseMirror (prosemirror-view 1.42.5)
       * redraws the block under the composition and Chromium restarts it:
       * IBus Hangul 한글 after "첫 문단" gave "첫 문단ㅎ한글". Tiptap's
       * TableOfContents skips composition transactions for the same reason
       * (ueberdosis/tiptap#7126, PR #7134); UniqueID has no such check and
       * no upstream issue. The id waits for the block's next edit outside a
       * composition (a syllable commit, a space, Enter). Tests:
       * packages/editor/test/unique-id-composition.test.ts and
       * apps/web/e2e-pending/workspace-wiki-ime.spec.ts. Drop the
       * composition check once UniqueID skips composition transactions
       * itself (those tests then pass without it). */
      filterTransaction: (tr) => !isChangeOrigin(tr) && !tr.getMeta("composition"),
    }),
    YChangeAttr,
    YChangeMark,
    StableTableOfContents.configure({
      anchorTypes: ["heading"],
      ...(typeof document === "undefined"
        ? {}
        : {
            scrollParent: () => document.getElementById("fv-main") ?? window,
          }),
    }),
    Placeholder.configure({
      includeChildren: true,
      placeholder: () => t("editor.placeholder"),
    }),
    Highlight.configure({ multicolor: true }),
    TextStyle,
    Color,
    TextAlign.configure({ types: ["heading", "paragraph"] }),
    Details.configure({ persist: true }),
    DetailsContent,
    DetailsSummary,
    TaskList,
    TaskItem.configure({ nested: true }),
    Callout,
    Mermaid,
    Extension.create({
      name: "mermaidFence",
      addInputRules() {
        return [
          new InputRule({
            find: /^```mermaid[\s\n]$/,
            handler: ({ chain, range }) => {
              chain()
                .deleteRange(range)
                .insertContent({
                  type: "mermaid",
                  attrs: { source: "" },
                })
                .run();
            },
          }),
        ];
      },
    }),
    MathBlock,
    MathInline,
    Attachment,
    MarkedEmoji.configure({
      suggestion: {
        items: emojiMenuItems,
        floatingUi: opts?.emojiSuggestionFloatingUi,
        ...(opts?.emojiSuggestionRender ? { render: opts.emojiSuggestionRender } : {}),
      },
    }),
    Mention,
    Embed,
  ];
}
