import type { Editor } from "@tiptap/core";
import { computed, shallowRef, watch } from "vue";
import { copyText } from "../clipboard.js";
import {
  ensureLanguage,
  HIGHLIGHT_MAX_CHARS,
  languageOfFence,
  parseFenceLanguage,
  parseHighlightLines,
} from "../lowlight.js";
import { useEditorState } from "./use-editor-state.js";

/** The language choices of the code-block chrome; "" is plain text. */
export const CODE_LANGUAGES = [
  "typescript",
  "javascript",
  "json",
  "css",
  "python",
  "diff",
  "",
] as const;

/** Line backgrounds: highlighted lines ({3-5}) and diff additions/removals. */
export const CODE_LINE_BACKGROUND = {
  meta: "color-mix(in oklch, var(--ring) 32%, transparent)",
  add: "color-mix(in oklch, var(--status-done) 28%, transparent)",
  del: "color-mix(in oklch, var(--destructive) 28%, transparent)",
} as const;

export type CodeLineKind = keyof typeof CODE_LINE_BACKGROUND;

/** The code block the caret is in. */
export type CodeBlockAtCaret = {
  id: string;
  editable: boolean;
  language: string;
  highlightLines: unknown;
  text: string;
};

/** View options of one code block; they are not document content. */
export type CodeChrome = { linenos: boolean; wrap: boolean; folded: boolean };

export const DEFAULT_CODE_CHROME: CodeChrome = { linenos: false, wrap: false, folded: false };

/** Blocks longer than this many lines can be folded. */
const FOLD_MIN_LINES = 9;

export function codeBlockAtCaret(editor: Editor): CodeBlockAtCaret | null {
  if (!editor.isActive("codeBlock")) return null;
  const $from = editor.state.selection.$from;
  const parent = $from.parent;
  const pos = $from.before($from.depth);
  const attrs = editor.getAttributes("codeBlock");
  return {
    id: String(parent.attrs.id ?? pos),
    editable: editor.isEditable,
    language: String(attrs.language ?? ""),
    highlightLines: attrs.highlightLines,
    text: parent.textContent,
  };
}

function asLineNumbers(value: unknown): number[] {
  if (typeof value === "string") return parseHighlightLines(value);
  if (Array.isArray(value)) {
    return value.filter((n): n is number => typeof n === "number" && Number.isInteger(n) && n > 0);
  }
  return [];
}

/** One line of the gutter beside the block: its number (or a blank) and its background. */
export type CodeGutterLine = { n: number; label: string; kind: CodeLineKind | null };

/** What the chrome shows for `block` (react/code-block-chrome.tsx): the
 * language to select, whether folding is offered, and the gutter lines
 * (null when neither line numbers nor highlights are shown). */
export function codeChromeView(
  block: CodeBlockAtCaret,
  chrome: CodeChrome,
): { language: string; foldable: boolean; lines: CodeGutterLine[] | null } {
  const parsed = parseFenceLanguage(block.language);
  const language = parsed.language || block.language;
  const rows = block.text.length === 0 ? [""] : block.text.split("\n");
  const fromAttr = asLineNumbers(block.highlightLines);
  const meta = new Set(fromAttr.length > 0 ? fromAttr : parsed.highlightLines);
  const isDiff = language === "diff";
  const paint = block.text.length <= HIGHLIGHT_MAX_CHARS && (meta.size > 0 || isDiff);
  const kindOf = (n: number, row: string): CodeLineKind | null => {
    if (!paint) return null;
    if (meta.has(n)) return "meta";
    if (!isDiff) return null;
    if (row.startsWith("+")) return "add";
    if (row.startsWith("-")) return "del";
    return null;
  };
  return {
    language,
    foldable: rows.length >= FOLD_MIN_LINES,
    lines:
      chrome.linenos || paint
        ? rows.map((row, i) => ({
            n: i + 1,
            label: chrome.linenos ? String(i + 1) : " ",
            kind: kindOf(i + 1, row),
          }))
        : null,
  };
}

/**
 * The code-block chrome's behaviour (react/code-block-chrome.tsx) for the
 * block the caret is in: language (an editor command, so it reaches peers),
 * copy with a visible failure, and the per-block view options line numbers,
 * fold and wrap, which the editor host carries as data attributes
 * (data-code-folded / data-code-wrap; the stylesheet reads them).
 */
export function useCodeBlockChrome(editor: Editor) {
  const block = useEditorState(editor, codeBlockAtCaret);
  const byId = shallowRef<Record<string, CodeChrome>>({});
  const copyFailedId = shallowRef<string | null>(null);
  const chrome = computed(() =>
    block.value ? (byId.value[block.value.id] ?? DEFAULT_CODE_CHROME) : DEFAULT_CODE_CHROME,
  );
  const view = computed(() => (block.value ? codeChromeView(block.value, chrome.value) : null));

  // Grammars load lazily; the highlighter repaints once one arrives.
  watch(
    block,
    (current) => {
      if (!current) return;
      const language = languageOfFence(current.language) || current.language;
      if (!language) return;
      void ensureLanguage(language, current.text);
    },
    { immediate: true },
  );

  // Wrap and fold live on the editor host as data attributes. The Vue host
  // binds them from codeChromeHostKey so a re-render cannot wipe them
  // (dataset writes on the component root do not survive Vue's patch).

  return {
    block,
    chrome,
    view,
    copyFailed: computed(() => block.value !== null && copyFailedId.value === block.value.id),
    patch(next: Partial<CodeChrome>): void {
      const current = block.value;
      if (!current) return;
      byId.value = { ...byId.value, [current.id]: { ...chrome.value, ...next } };
    },
    setLanguage(value: string): void {
      if (editor.isDestroyed || !editor.isEditable || editor.view.composing) return;
      editor.commands.updateAttributes("codeBlock", { language: languageOfFence(value) });
    },
    copy(): void {
      const current = block.value;
      if (!current) return;
      copyFailedId.value = null;
      void copyText(current.text).catch(() => {
        copyFailedId.value = current.id;
      });
    },
  };
}
