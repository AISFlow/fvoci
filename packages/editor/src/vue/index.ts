// The Vue editor host and its node views (@fvoci/editor/vue).
export { default as FvociEditor } from "./FvociEditor.vue";
export { default as SafeHtml } from "./SafeHtml.vue";
export { asSafeHtml, type SafeHtml as SafeHtmlString } from "../safe-html.js";
export {
  attachmentBridgeKey,
  type CodeChromeHost,
  codeChromeHostKey,
  entityResolverKey,
  type UrlEmbedComponent,
  urlEmbedKey,
} from "./keys.js";
export { VUE_NODE_VIEWS } from "./node-views.js";
export type { Editor as TiptapEditor } from "@tiptap/core";
export type { EntityResolver, EntitySnapshot, MentionEntity } from "../entities.js";
export type { MentionHit, MentionLoader } from "../editor-extensions.js";
export { collabCaretRender, type FvociCollabUser } from "../editor-extensions.js";

// The editing controls' behaviour; the web app renders them (Nuxt UI).
export { sameValue, useEditorState } from "./use-editor-state.js";
export {
  type BlockMenuAnchor,
  type GutterBlock,
  type GutterHandle,
  keyboardBlockPos,
  useBlockGutter,
} from "./block-gutter.js";
export {
  boxInHost,
  type TableCaret,
  type TableHandlesBox,
  tableCaret,
  useTableHandles,
} from "./table-handles.js";
export {
  CODE_LANGUAGES,
  CODE_LINE_BACKGROUND,
  type CodeBlockAtCaret,
  type CodeChrome,
  type CodeGutterLine,
  codeBlockAtCaret,
  codeChromeView,
  useCodeBlockChrome,
} from "./code-block-chrome.js";
export { leaveMenu, restoreNativeSelection } from "./menu.js";
export { focusFirstMenuItem, moveMenuFocus } from "../menu-roving.js";
export { overlayOwner } from "../overlay-owner.js";
export { copyText } from "../clipboard.js";
export {
  convertBlock,
  deleteBlock,
  duplicateBlock,
  insertSlashHere,
  moveBlock,
} from "../gutter-actions.js";
export {
  addColumn,
  addRow,
  deleteCurrentTable,
  equalizeColumns,
  mergeSelectedCells,
  setCellAlign,
  setCellBackground,
  splitSelectedCells,
  toggleTableHeaderColumn,
  toggleTableHeaderRow,
} from "../table-actions.js";
