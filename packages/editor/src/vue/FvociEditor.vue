<script setup lang="ts">
import { t } from "@fvoci/i18n";
import type { HocuspocusProvider } from "@hocuspocus/provider";
import { getSchema, type Editor, type MappablePosition } from "@tiptap/core";
import {
  AllSelection,
  type EditorState,
  type SelectionBookmark,
  TextSelection,
} from "@tiptap/pm/state";
import type { Mark } from "@tiptap/pm/model";
import { CellSelection } from "@tiptap/pm/tables";
import type { EditorView } from "@tiptap/pm/view";
import BubbleMenu from "@tiptap/extension-bubble-menu";
import DragHandle from "@tiptap/extension-drag-handle";
import { EditorContent, useEditor } from "@tiptap/vue-3";
import {
  markRaw,
  computed,
  nextTick,
  onBeforeUnmount,
  onMounted,
  provide,
  reactive,
  ref,
  shallowRef,
  useSlots,
  useTemplateRef,
  watch,
} from "vue";
import type * as Y from "yjs";
import type { AttachmentBlockBridge, AttachmentUploadResult } from "../attachment-model.js";
import {
  createFvociEditorExtensions,
  createFvociEditorProps,
  FILE_UPLOAD_META,
  type FvociCollabUser,
  type MentionLoader,
  uploadAnchor,
} from "../editor-extensions.js";
import type { EntityResolver } from "../entities.js";
import { overlayOwner } from "../overlay-owner.js";
import { selectAllEscape, selectAllStep } from "../table-actions.js";
import { moveBlock } from "../gutter-actions.js";
import {
  rawEditorPreflight,
  SourceModeSession,
  type SourceCapture,
  type SourceProposal,
} from "../source-mode.js";
import { copyText } from "../clipboard.js";
import { tiptapDocToMd } from "../md.js";
import { yDocToTiptapJson } from "../collab-tiptap.js";
import { editorModePreview } from "./editor-mode-preview.js";
import SafeHtml from "./SafeHtml.vue";
import AttachmentBlock from "./AttachmentBlock.vue";
import { keyboardBlockPos, type GutterBlock, type GutterHandle } from "./block-gutter.js";
import {
  attachmentBridgeKey,
  type CodeChromeHost,
  codeChromeHostKey,
  entityResolverKey,
  type UrlEmbedComponent,
  urlEmbedKey,
} from "./keys.js";
import { VUE_NODE_VIEWS } from "./node-views.js";

// The collaborative FVOCI editor for Vue pages (react/fvoci-editor.tsx is the
// React host): the shared extension list (createFvociEditorExtensions, the
// only schema) with the Vue node views, bound to the room's Y.Doc and
// provider. The document lives only in Yjs; nothing here copies it out.
// ydoc, provider and user are fixed for the component's life: the page keys
// it by the room's socket generation, so a new provider mounts a new editor.
// The host supplies the toolbar, the selection bubble and the editing
// controls (block gutter, table handles, mobile toolbar, code-block chrome)
// through slots.
const props = defineProps<{
  ydoc: Y.Doc;
  provider: HocuspocusProvider;
  user: FvociCollabUser;
  editable: boolean;
  ariaLabel?: string;
  workspaceSlug?: string | null;
  /** Upload hooks of attachment blocks and drop/paste uploads; none disables uploading. */
  attachmentBridge?: AttachmentBlockBridge | null;
  /** Renders URL embeds (the unfurl card). */
  urlEmbed?: UrlEmbedComponent | null;
  entityResolver?: EntityResolver | null;
  mentionItems?: MentionLoader;
  /** Host-owned monotonic actor/room generation, including ABA transitions. */
  modeScope?: string | number;
  /** Existing scoped durable ACK barrier; rejects failed/old/wrong ACKs. */
  waitForSave?: () => Promise<boolean>;
}>();
/** The live editor once it exists, and null when it is torn down. */
type SourceDraftState = Readonly<{
  owner: object;
  phase: "activate" | "change" | "retire";
  scope: string | number | undefined;
  dirty: boolean;
  stale: boolean;
  composing: boolean;
}>;
const emit = defineEmits<{
  ready: [editor: Editor | null];
  "mode-change": [mode: EditorMode];
  "source-dirty": [state: SourceDraftState];
}>();
defineSlots<{
  toolbar?(props: { editor: Editor }): unknown;
  bubble?(props: { editor: Editor }): unknown;
  /** Rendered in the editor host after the content (react/fvoci-editor.tsx
   * renders its gutter, table handles, mobile toolbar and code-block chrome
   * there). `gutter` is the block drag handle the editor starts with; its
   * buttons render into `gutter.element`. */
  controls?(props: { editor: Editor; gutter: GutterHandle; editable: boolean }): unknown;
}>();
const slots = useSlots();

type EditorMode = "rich" | "block" | "markdown" | "preview";
const mode = ref<EditorMode>("rich");
const modes = [
  { value: "rich", label: t("editor.mode.rich") },
  { value: "block", label: t("editor.mode.block") },
  { value: "markdown", label: t("editor.mode.markdown") },
  { value: "preview", label: t("editor.mode.preview") },
] as const;
const richVisible = computed(() => mode.value === "rich" || mode.value === "block");
const sourceField = useTemplateRef<HTMLTextAreaElement>("sourceField");
const sourceComposing = ref(false);
const capture = shallowRef<SourceCapture | null>(null);
const proposal = shallowRef<SourceProposal | null>(null);
const draftDirty = ref(false);
const sourceStale = ref(false);
const draftOwner = markRaw({});
let draftMounted = false;
const sourceDraftState = computed<SourceDraftState>(() => ({
  owner: draftOwner,
  phase: "change",
  scope: props.modeScope,
  dirty: draftDirty.value,
  stale: sourceStale.value,
  composing: sourceComposing.value,
}));
onMounted(() => {
  draftMounted = true;
  emit("source-dirty", { ...sourceDraftState.value, phase: "activate" });
});
watch(
  sourceDraftState,
  (state) => {
    if (draftMounted) emit("source-dirty", state);
  },
  { flush: "sync" },
);
function discardSourceDraft(): void {
  if (sourceComposing.value) return;
  if (editor.value) refreshSource();
  else {
    // Raw-unsupported retirement still permits discarding this transient text.
    // It does not bind, coerce or replace the host's live fragment.
    if (sourceField.value) sourceField.value.value = "";
    capture.value = null;
    proposal.value = null;
    draftDirty.value = false;
    sourceStale.value = false;
    modeError.value = null;
  }
}
defineExpose({ discardSourceDraft, sourceDraftState });
const modeError = ref<string | null>(null);
const preview = shallowRef<Awaited<ReturnType<typeof editorModePreview>> | null>(null);
const activeBlockPos = ref(-1);
let modeLifetime = 0;
let scopeEpoch = 0;
let bookmark: SelectionBookmark | null = null;
let storedMarks: readonly Mark[] | null = null;
let restoreEditorFocus = false;
const sourceSession = markRaw(
  new SourceModeSession(
    props.ydoc,
    () => scopeEpoch,
    () => props.editable,
    inspectRawBeforeBinding,
  ),
);

/** Registered before ySync's observer. Unsupported schema data retires this
 * binding through its public lifecycle before the SDK can repair it. The
 * host's same Y.Doc/provider remain authoritative. */
function inspectRawBeforeBinding(): void {
  const current = editor.value;
  if (!current || current.isDestroyed) return;
  const issues = rawEditorPreflight(props.ydoc, current.schema);
  if (!issues.length) return;
  previewAbort?.abort();
  rawIssues.value = issues;
  scopeEpoch++;
  modeLifetime++;
  sourceStale.value = Boolean(capture.value);
  current.destroy();
  editor.value = undefined;
  anchors.clear();
  uploads.value = [];
  emit("ready", null);
}
watch(
  [
    () => props.modeScope,
    () => props.user.id,
    () => props.ydoc,
    () => props.provider,
    () => props.editable,
  ],
  (next, previous) => {
    previewAbort?.abort();
    scopeEpoch++;
    modeLifetime++;
    sourceStale.value = Boolean(capture.value);
    if (next[1] !== previous[1] || next[2] !== previous[2] || next[3] !== previous[3]) {
      // The host owns forced actor/room cleanup. If fixed-lifetime props change
      // before unmount, retire this old binding and private transient content
      // synchronously; a different actor must never inherit the prior draft.
      if (sourceField.value) sourceField.value.value = "";
      capture.value = null;
      proposal.value = null;
      draftDirty.value = false;
      sourceStale.value = false;
      preview.value = null;
      sourceComposing.value = false;
      const current = editor.value;
      current?.destroy();
      editor.value = undefined;
      uploads.value = [];
      anchors.clear();
      emit("ready", null);
    } else if (mode.value === "preview" && editor.value) {
      // A same-actor readable permission/scope change retires old resolver
      // work, then renders the same authorized live document anew.
      refreshPreview(editor.value);
    }
  },
  { flush: "sync" },
);

function onSourceInput(event: Event): void {
  if (!(event.target instanceof HTMLTextAreaElement)) return;
  draftDirty.value = event.target.value !== capture.value?.source;
  proposal.value = null;
  modeError.value = null;
}

function sourceBlocked(): boolean {
  return sourceComposing.value || Boolean(editor.value?.view.composing);
}

function moveSelectedBlock(direction: -1 | 1): void {
  const current = editor.value;
  if (!current || !props.editable || sourceBlocked()) return;
  // Read the current selection at execution time, never a stored drag position.
  const position = keyboardBlockPos(current);
  if (position >= 0) moveBlock(current, position, direction);
}

function onSourceKeyDown(event: KeyboardEvent): void {
  // Native IME compatibility, identical to the rich keyboard entry boundary.
  // eslint-disable-next-line @typescript-eslint/no-deprecated
  if (event.isComposing || event.keyCode === 229) sourceComposing.value = true;
}

let previewAbort: AbortController | null = null;
function refreshPreview(current: Editor): void {
  previewAbort?.abort();
  const controller = new AbortController();
  previewAbort = controller;
  preview.value = null;
  const entry = sourceSession.capture(current.state.doc);
  const lifetime = modeLifetime;
  void editorModePreview(
    current,
    { attachmentBridge: props.attachmentBridge, entityResolver: props.entityResolver },
    controller.signal,
  ).then(
    (html) => {
      if (
        controller.signal.aborted ||
        current.isDestroyed ||
        lifetime !== modeLifetime ||
        mode.value !== "preview" ||
        !sourceSession.isCurrent(entry)
      )
        return;
      preview.value = html;
    },
    (error: unknown) => {
      if (!controller.signal.aborted && lifetime === modeLifetime)
        modeError.value = error instanceof Error ? error.message : t("editor.mode.copyFailed");
    },
  );
}

async function changeMode(next: EditorMode): Promise<void> {
  const current = editor.value;
  if (!current || sourceBlocked() || mode.value === next) return;
  if (richVisible.value && (next === "markdown" || next === "preview")) {
    bookmark = current.state.selection.getBookmark();
    storedMarks = current.state.storedMarks;
    restoreEditorFocus = current.view.hasFocus();
  }
  const restore = !richVisible.value && (next === "rich" || next === "block");
  previewAbort?.abort();
  const lifetime = ++modeLifetime;
  modeError.value = null;
  mode.value = next;
  // Visibility changes do not revoke editor authorization or notify node-view
  // permission listeners: those can finalize their own transient rich drafts.
  // Native source keys are kept outside rich controls by the existing guards.
  if (next === "preview") refreshPreview(current);
  if (next === "markdown" && !capture.value) refreshSource();
  emit("mode-change", next);
  await nextTick();
  if (lifetime !== modeLifetime || current.isDestroyed) return;
  if (next === "markdown") sourceField.value?.focus();
  else if (restore && bookmark) {
    const selection = bookmark.resolve(current.state.doc);
    current.view.dispatch(current.state.tr.setSelection(selection).setStoredMarks(storedMarks));
    if (restoreEditorFocus) current.view.focus();
  }
}

function refreshSource(): void {
  const current = editor.value;
  if (!current || sourceBlocked()) return;
  capture.value = sourceSession.capture(current.state.doc);
  // Uncontrolled field: peer updates and Vue renders never overwrite typing.
  if (sourceField.value) sourceField.value.value = capture.value.source;
  draftDirty.value = false;
  sourceStale.value = false;
  proposal.value = null;
  modeError.value = null;
}

function applySource(): void {
  const current = editor.value;
  const entry = capture.value;
  if (!current || !entry || !sourceField.value || sourceBlocked() || !props.editable) return;
  proposal.value = sourceSession.prepare(entry, sourceField.value.value, current.state);
  sourceStale.value = proposal.value.status === "stale";
  if (proposal.value.status === "noop") {
    refreshSource();
    return;
  }
  if (proposal.value.status !== "ready") return;
  if (!sourceSession.apply(proposal.value, current)) {
    sourceStale.value = true;
    return;
  }
  refreshSource();
}

async function copyLiveSource(): Promise<void> {
  const barrier = props.waitForSave;
  const current = editor.value;
  if (!barrier || !current || sourceBlocked()) return;
  const entry = sourceSession.capture(current.state.doc);
  const lifetime = modeLifetime;
  modeError.value = null;
  try {
    if (
      !(await barrier()) ||
      lifetime !== modeLifetime ||
      !sourceSession.isCurrent(entry) ||
      current.isDestroyed
    )
      throw new Error(t("editor.mode.copyStale"));
    await copyText(tiptapDocToMd(yDocToTiptapJson(props.ydoc)));
  } catch (error) {
    if (lifetime === modeLifetime)
      modeError.value = error instanceof Error ? error.message : t("editor.mode.copyFailed");
  }
}

provide(attachmentBridgeKey, props.attachmentBridge ?? null);
provide(urlEmbedKey, props.urlEmbed ?? null);
provide(entityResolverKey, props.entityResolver ?? null);
const codeChromeHost = reactive<CodeChromeHost>({ wrap: null, folded: null });
provide(codeChromeHostKey, codeChromeHost);

/* Drop/paste uploads waiting for their attachment node. */
const uploads = shallowRef<Array<{ key: string; file: File }>>([]);
const anchors = new Map<string, MappablePosition>();

function queueUploads(current: Editor, files: File[], pos: number): void {
  if (!props.attachmentBridge || !props.editable || rawIssues.value.length) return;
  const queued = files.map((file) => {
    const key = crypto.randomUUID();
    anchors.set(key, uploadAnchor(current, pos));
    return { key, file };
  });
  uploads.value = [...uploads.value, ...queued];
}

function dropUpload(key: string): void {
  anchors.delete(key);
  uploads.value = uploads.value.filter((item) => item.key !== key);
}

function insertUploaded(key: string, result: AttachmentUploadResult): void {
  const current = editor.value;
  const anchor = anchors.get(key);
  if (!current || current.isDestroyed || !anchor || !props.editable) return;
  current
    .chain()
    .setMeta(FILE_UPLOAD_META, key)
    .insertContentAt(
      anchor.position,
      { type: "attachment", attrs: result },
      { updateSelection: false },
    )
    .run();
  dropUpload(key);
}

/* WHY: the selection bubble is one of the extensions the editor starts with,
 * not a plugin a menu component registers after mount (@tiptap/vue-3's
 * BubbleMenu). That Editor answers `state` from a reactive copy it updates only
 * after registerPlugin returns, but ProseMirror rebuilds the plugin views inside
 * the call and y-sync dispatches from its new view there: the dispatch applied
 * to the pre-registration state and swapped the plugin set back, rebuilding the
 * views again (twice for some). Tiptap's collaboration undo view then brought
 * the Yjs UndoManager back without itself among its tracked origins, so undo
 * worked and redo never did (e2e/workspace-wiki-vue-flow.spec.ts). The bubble
 * content renders into this element through a Teleport. */
const bubble = markRaw(document.createElement("div"));
bubble.className = "fvoci-bubble";
bubble.dataset.fvociBubble = "";

/* WHY: the block drag handle is an extension the editor starts with, for the
 * reason the bubble is (above): @tiptap/extension-drag-handle-vue-3 registers
 * its plugin after mount. Its buttons render into this element through the
 * controls slot's Teleport; the plugin positions and shows it, and moves the
 * dragged block with a ProseMirror drop, which Yjs carries to peers. */
const gutterElement = markRaw(document.createElement("div"));
gutterElement.className = "fvoci-gutter";
gutterElement.style.visibility = "hidden";
gutterElement.style.position = "absolute";
gutterElement.dataset.dragging = "false";
const gutterBlock = shallowRef<GutterBlock>({ node: null, pos: -1 });
const gutter: GutterHandle = markRaw({ element: gutterElement, block: gutterBlock });

/* WHY: #738 — Tiptap 기본값은 <style data-tiptap-style> 을 head 에 꽂는다. style-src 는 'self' 와
 * 셸 인라인 블록의 빌드 시점 해시뿐이라 그 <style> 은 차단된다 — 규칙은 react/editor.css 에 있다. */
const editorExtensions = [
  ...createFvociEditorExtensions({
    ydoc: props.ydoc,
    nodeViews: VUE_NODE_VIEWS,
    mentionItems: () => props.mentionItems,
    entityResolver: () => props.entityResolver,
    workspaceSlug: () => props.workspaceSlug,
    uploads: { anchors, queue: queueUploads },
    provider: props.provider,
    user: props.user,
  }),
  ...(slots.controls
    ? [
        DragHandle.configure({
          render: () => gutterElement,
          nested: true,
          // The plugin passes the block's position too (the option's type leaves it out).
          onNodeChange: (change) => {
            const pos = (change as { pos?: number }).pos;
            gutterBlock.value = { node: change.node, pos: typeof pos === "number" ? pos : -1 };
          },
        }),
      ]
    : []),
  ...(slots.bubble
    ? [
        BubbleMenu.configure({
          element: bubble,
          updateDelay: 0,
          options: { placement: "bottom" },
          appendTo: () => bubbleOwner(),
          shouldShow: bubbleShouldShow,
        }),
      ]
    : []),
];
const rawIssues = shallowRef(rawEditorPreflight(props.ydoc, getSchema(editorExtensions)));
const editor = rawIssues.value.length
  ? shallowRef<import("@tiptap/vue-3").Editor>()
  : useEditor({
      injectCSS: false,
      editable: props.editable,
      extensions: editorExtensions,
      editorProps: {
        ...createFvociEditorProps(props.ariaLabel),
        handleClick: settleNativeTextClick,
        handleDOMEvents: { keyup: settleNativeKeyboardSelection },
      },
    });

/* A native click places the DOM caret before selectionchange records it in PM.
 * A remote Yjs update in that gap restores PM's previous selection over the
 * clicked caret. Record an ordinary collapsed text click at PM's mouseup
 * boundary, before a subsequent remote update can snapshot the old position.
 * Returning false leaves native click handling and other plugins in control;
 * this changes only the selection, using PM's pointer transaction semantics. */
function settleNativeTextClick(view: EditorView, pos: number, event: MouseEvent): boolean {
  if (
    !view.editable ||
    view.composing ||
    !view.hasFocus() ||
    event.button !== 0 ||
    event.shiftKey ||
    event.ctrlKey ||
    event.metaKey ||
    event.altKey
  )
    return false;
  const native = view.dom.ownerDocument.getSelection();
  if (!native?.isCollapsed || !native.anchorNode || !view.dom.contains(native.anchorNode))
    return false;
  if (event.target instanceof Element && event.target.closest('[contenteditable="false"]'))
    return false;
  const $pos = view.state.doc.resolve(pos);
  if (!$pos.parent.isTextblock) return false;
  const selection = TextSelection.create(view.state.doc, pos);
  if (!view.state.selection.eq(selection)) {
    view.dispatch(view.state.tr.setSelection(selection).setMeta("pointer", true));
  }
  return false;
}

/* Native navigation changes the DOM selection before selectionchange reaches
 * PM. Its pending focus repair can otherwise restore the previous caret or
 * range in that gap. Record the completed selection at keyup using public APIs;
 * leave composition, cell/node selections and native event handling alone. */
function settleNativeKeyboardSelection(view: EditorView, event: KeyboardEvent): boolean {
  if (
    !view.editable ||
    view.composing ||
    event.isComposing ||
    !view.hasFocus() ||
    ![
      "Home",
      "End",
      "ArrowLeft",
      "ArrowRight",
      "ArrowUp",
      "ArrowDown",
      "PageUp",
      "PageDown",
    ].includes(event.key) ||
    !(view.state.selection instanceof TextSelection)
  )
    return false;
  const native = view.dom.ownerDocument.getSelection();
  if (
    !native ||
    !native.anchorNode ||
    !native.focusNode ||
    !view.dom.contains(native.anchorNode) ||
    !view.dom.contains(native.focusNode)
  )
    return false;
  for (const node of [native.anchorNode, native.focusNode]) {
    const element = node instanceof Element ? node : node.parentElement;
    const leaf = element?.closest('[contenteditable="false"]');
    if (leaf && leaf !== view.dom && view.dom.contains(leaf)) return false;
  }
  const anchor = view.posAtDOM(native.anchorNode, native.anchorOffset);
  const head = view.posAtDOM(native.focusNode, native.focusOffset);
  if (
    !view.state.doc.resolve(anchor).parent.isTextblock ||
    !view.state.doc.resolve(head).parent.isTextblock
  )
    return false;
  const selection = TextSelection.create(view.state.doc, anchor, head);
  if (!view.state.selection.eq(selection)) view.dispatch(view.state.tr.setSelection(selection));
  return false;
}

watch(
  () => props.editable,
  (editable) => editor.value?.setEditable(editable),
);

/* WHY: #571 — 열린 오버레이가 Escape 를 먹는다. 에디터까지 올라가면 selectAllEscape 가 함께 돈다. */
const OVERLAY_SELECTOR =
  ".fvoci-block-menu, .fvoci-ui-popover-content, .fvoci-ui-dropdown-content, [data-reka-popper-content-wrapper]";

function isNarrowViewport(): boolean {
  return window.matchMedia("(max-width: 47.999rem)").matches;
}

/* The bubble shows for a non-empty text, cell or whole-document selection, not on narrow screens. */
function bubbleShouldShow({ state }: { state: EditorState }): boolean {
  return (
    richVisible.value &&
    (state.selection instanceof TextSelection ||
      state.selection instanceof CellSelection ||
      (state.selection instanceof AllSelection && state.doc.textContent.length > 0)) &&
    !state.selection.empty &&
    !isNarrowViewport()
  );
}

function isGuardedTextField(target: EventTarget | null, host: HTMLElement | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  if (target.closest("input, textarea, select")) return true;
  const pm = host?.querySelector(".ProseMirror");
  if (pm?.contains(target)) return false;
  if (target.closest("[role='textbox']")) return true;
  return target.isContentEditable;
}

const host = useTemplateRef<HTMLDivElement>("host");

// WHY: 문서·태스크 본문은 페이지당 편집기 하나다 — 전역 keydown 은 이 호스트 하나만 본다.
watch(editor, (current, _previous, onCleanup) => {
  if (!current) return;
  emit("ready", current);
  activeBlockPos.value = keyboardBlockPos(current);
  const onTransaction = ({
    transaction,
  }: {
    transaction: import("@tiptap/pm/state").Transaction;
  }) => {
    activeBlockPos.value = keyboardBlockPos(current);
    if (bookmark && transaction.docChanged) bookmark = bookmark.map(transaction.mapping);
    if (capture.value && transaction.docChanged) sourceStale.value = true;
    if (mode.value === "preview" && transaction.docChanged) refreshPreview(current);
  };
  current.on("transaction", onTransaction);
  const onKeyDown = (event: KeyboardEvent) => {
    const element = host.value;
    if (!element?.isConnected) return;
    if (!richVisible.value) return;
    // Native IME compatibility: some composition keys report 229 before isComposing.
    // Remove only after a supported replacement passes the native IME regressions.
    // eslint-disable-next-line @typescript-eslint/no-deprecated
    if (event.isComposing || current.view.composing || event.keyCode === 229) return;
    if (isGuardedTextField(event.target, element)) return;
    if (event.key === "Escape") {
      if (document.querySelector(OVERLAY_SELECTOR)) return;
      if (selectAllEscape(current)) event.preventDefault();
      return;
    }
    if (event.key !== "a" && event.key !== "A") return;
    if (!(event.metaKey || event.ctrlKey) || event.altKey) return;
    event.preventDefault();
    selectAllStep(current);
  };
  document.addEventListener("keydown", onKeyDown, true);
  onCleanup(() => {
    document.removeEventListener("keydown", onKeyDown, true);
    current.off("transaction", onTransaction);
  });
});

onBeforeUnmount(() => {
  draftMounted = false;
  emit("source-dirty", { ...sourceDraftState.value, phase: "retire" });
  modeLifetime++;
  previewAbort?.abort();
  sourceSession.destroy();
  emit("ready", null);
});

/* Padding below the last block focuses the end, as in the React host. */
function onHostMouseDown(event: MouseEvent): void {
  const current = editor.value;
  if (!current || !richVisible.value || event.target !== host.value) return;
  event.preventDefault();
  current.commands.focus("end");
}

function bubbleOwner(): HTMLElement {
  const current = editor.value;
  return current ? overlayOwner(current.view.dom) : document.body;
}
</script>

<template>
  <div v-if="editor" class="fvoci-mode-controls" role="group" :aria-label="t('editor.mode.group')">
    <button
      v-for="item in modes"
      :key="item.value"
      type="button"
      :data-editor-mode="item.value"
      :aria-pressed="mode === item.value"
      :disabled="sourceComposing"
      @mousedown.prevent
      @click="changeMode(item.value)"
      >{{ item.label }}</button
    >
  </div>
  <div v-show="richVisible"><slot v-if="editor" name="toolbar" :editor="editor" /></div>
  <div v-if="mode === 'block'" class="fvoci-mode-controls" :aria-label="t('editor.mode.block')">
    <button
      type="button"
      :disabled="!editable || activeBlockPos < 0"
      @mousedown.prevent
      @click="moveSelectedBlock(-1)"
      >{{ t("editor.mode.moveUp") }}</button
    >
    <button
      type="button"
      :disabled="!editable || activeBlockPos < 0"
      @mousedown.prevent
      @click="moveSelectedBlock(1)"
      >{{ t("editor.mode.moveDown") }}</button
    >
  </div>
  <div
    ref="host"
    class="fvoci-editor relative min-h-[16rem]"
    :data-editor-mode-active="mode"
    :data-code-wrap="
      codeChromeHost.wrap === null ? undefined : codeChromeHost.wrap ? 'true' : 'false'
    "
    :data-code-folded="
      codeChromeHost.folded === null ? undefined : codeChromeHost.folded ? 'true' : 'false'
    "
    @mousedown="onHostMouseDown"
  >
    <div v-if="rawIssues.length" role="alert" class="fvoci-mode-warning">
      <p>{{ t("editor.mode.rawReadOnly") }}</p>
      <ul
        ><li v-for="(item, index) in rawIssues.slice(0, 20)" :key="index"
          >{{ item.path }} · {{ item.id ?? "ID 없음" }} · {{ item.field }}: {{ item.reason }}</li
        ></ul
      >
      <p v-if="rawIssues.length > 20">총 {{ rawIssues.length }}개 항목</p>
    </div>
    <Teleport v-if="editor && $slots.bubble" :to="bubble">
      <slot name="bubble" :editor="editor" />
    </Teleport>
    <div v-show="richVisible"><EditorContent :editor="editor" /></div>
    <div v-show="mode === 'markdown'" class="fvoci-source-panel">
      <p>{{ t("editor.mode.generated") }}</p>
      <textarea
        ref="sourceField"
        :aria-label="t('editor.mode.sourceLabel')"
        :readonly="!editable || rawIssues.length > 0"
        spellcheck="false"
        @input="onSourceInput"
        @keydown="onSourceKeyDown"
        @compositionstart="sourceComposing = true"
        @compositionend="sourceComposing = false"
      />
      <p v-if="sourceStale" role="status">{{ t("editor.mode.stale") }}</p>
      <p v-else-if="draftDirty" role="status">{{ t("editor.mode.draft") }}</p>
      <div
        v-if="proposal && (proposal.status === 'loss' || proposal.status === 'invalid')"
        role="alert"
        tabindex="0"
        class="fvoci-mode-warning"
      >
        <p>{{ t("editor.mode.loss") }}</p>
        <ul
          ><li v-for="(item, index) in proposal.diagnostics" :key="index"
            >{{ item.path }} · {{ item.id ?? "ID 없음" }} · {{ item.field }}: {{ item.reason }}</li
          ></ul
        >
        <p v-if="proposal.total > proposal.diagnostics.length">총 {{ proposal.total }}개 항목</p>
      </div>
      <div class="fvoci-mode-controls">
        <button
          type="button"
          :disabled="!editor || !editable || !draftDirty || sourceStale || sourceComposing"
          @click="applySource"
          >적용</button
        >
        <button type="button" :disabled="sourceComposing" @click="discardSourceDraft">{{
          t("editor.mode.cancel")
        }}</button>
        <button
          v-if="waitForSave"
          type="button"
          :disabled="sourceComposing"
          @click="copyLiveSource"
          >{{ t("editor.mode.copy") }}</button
        >
      </div>
    </div>
    <p v-if="editor && mode === 'preview' && !preview" role="status">{{
      t("editor.embed.loading")
    }}</p>
    <SafeHtml
      v-if="preview"
      v-show="mode === 'preview'"
      :html="preview"
      class="fvoci-mode-preview"
    />
    <p v-if="modeError" role="alert">{{ modeError }}</p>
    <div
      v-if="uploads.length > 0"
      data-fvoci-uploads=""
      class="sticky bottom-0 z-10 flex flex-col gap-1 bg-default py-1"
    >
      <AttachmentBlock
        v-for="item in uploads"
        :key="item.key"
        :initial-file="item.file"
        :block-props="{
          id: '',
          name: item.file.name,
          image: false,
          width: 100,
          align: 'center',
          caption: '',
          previewWidth: 0,
          previewHeight: 0,
        }"
        :read-only="!editable || rawIssues.length > 0"
        @remove="dropUpload(item.key)"
        @uploaded="insertUploaded(item.key, $event)"
      />
    </div>
    <div v-show="richVisible"
      ><slot
        v-if="editor && $slots.controls"
        name="controls"
        :editor="editor"
        :gutter="gutter"
        :editable="editable"
    /></div>
  </div>
</template>

<style scoped>
.fvoci-mode-controls {
  display: flex;
  flex-wrap: wrap;
  gap: 0.5rem;
  margin-block: 0.5rem;
  font-size: 0.875rem;
  line-height: 1.5;
}
.fvoci-mode-controls button {
  border: 1px solid var(--border);
  border-radius: 0.5rem;
  padding: 0.375rem 0.75rem;
  color: var(--foreground);
  background: var(--background);
}
.fvoci-mode-controls button[aria-pressed="true"] {
  border-color: var(--primary);
  color: var(--primary);
}
.fvoci-mode-controls button:focus-visible,
.fvoci-source-panel textarea:focus-visible {
  outline: 2px solid var(--primary);
  outline-offset: 2px;
}
.fvoci-mode-controls button:disabled {
  opacity: 0.5;
}
.fvoci-source-panel,
.fvoci-mode-preview {
  font-size: 1rem;
  line-height: 1.6;
  word-break: keep-all;
  overflow-wrap: anywhere;
}
.fvoci-source-panel textarea {
  display: block;
  width: 100%;
  min-height: 20rem;
  border: 1px solid var(--border);
  border-radius: 0.5rem;
  padding: 0.75rem;
  font-family: var(--font-mono, monospace);
  font-size: 1rem;
  line-height: 1.6;
  color: var(--foreground);
  background: var(--background);
  resize: vertical;
}
.fvoci-mode-warning {
  padding: 0.75rem;
  border: 1px solid var(--border);
  border-radius: 0.5rem;
}
.fvoci-mode-preview {
  overflow-x: auto;
}
</style>
