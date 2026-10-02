import type { Editor } from "@tiptap/core";
import { asSafeHtml, type SafeHtml } from "../safe-html.js";
import { sanitizeRenderedHtml } from "../sanitize.js";

/** Live schema rendering, sanitized exactly once at the existing trust boundary.
 * Does not parse or apply the uncommitted Markdown draft. */
export function editorModePreview(editor: Editor): SafeHtml {
  return sanitizeEditorModePreview(editor.getHTML());
}

export function sanitizeEditorModePreview(html: string): SafeHtml {
  return asSafeHtml(sanitizeRenderedHtml(html));
}
