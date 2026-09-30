<script setup lang="ts">
import type { HocuspocusProvider } from "@hocuspocus/provider";
import type { Editor, MappablePosition } from "@tiptap/core";
import { AllSelection, type EditorState, TextSelection } from "@tiptap/pm/state";
import { CellSelection } from "@tiptap/pm/tables";
import type { EditorView } from "@tiptap/pm/view";
import BubbleMenu from "@tiptap/extension-bubble-menu";
import DragHandle from "@tiptap/extension-drag-handle";
import { EditorContent, useEditor } from "@tiptap/vue-3";
import {
  markRaw,
  onBeforeUnmount,
  provide,
  reactive,
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
import AttachmentBlock from "./AttachmentBlock.vue";
import type { GutterBlock, GutterHandle } from "./block-gutter.js";
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
}>();
/** The live editor once it exists, and null when it is torn down. */
const emit = defineEmits<{ ready: [editor: Editor | null] }>();
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

provide(attachmentBridgeKey, props.attachmentBridge ?? null);
provide(urlEmbedKey, props.urlEmbed ?? null);
provide(entityResolverKey, props.entityResolver ?? null);
const codeChromeHost = reactive<CodeChromeHost>({ wrap: null, folded: null });
provide(codeChromeHostKey, codeChromeHost);

/* Drop/paste uploads waiting for their attachment node. */
const uploads = shallowRef<Array<{ key: string; file: File }>>([]);
const anchors = new Map<string, MappablePosition>();

function queueUploads(current: Editor, files: File[], pos: number): void {
  if (!props.attachmentBridge) return;
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
  if (!current || current.isDestroyed || !anchor) return;
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
const editor = useEditor({
  injectCSS: false,
  editable: props.editable,
  extensions: [
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
  ],
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
  const onKeyDown = (event: KeyboardEvent) => {
    const element = host.value;
    if (!element?.isConnected) return;
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
  onCleanup(() => document.removeEventListener("keydown", onKeyDown, true));
});

onBeforeUnmount(() => emit("ready", null));

/* Padding below the last block focuses the end, as in the React host. */
function onHostMouseDown(event: MouseEvent): void {
  const current = editor.value;
  if (!current || event.target !== host.value) return;
  event.preventDefault();
  current.commands.focus("end");
}

function bubbleOwner(): HTMLElement {
  const current = editor.value;
  return current ? overlayOwner(current.view.dom) : document.body;
}
</script>

<template>
  <slot v-if="editor" name="toolbar" :editor="editor" />
  <div
    ref="host"
    class="fvoci-editor relative min-h-[16rem]"
    :data-code-wrap="
      codeChromeHost.wrap === null ? undefined : codeChromeHost.wrap ? 'true' : 'false'
    "
    :data-code-folded="
      codeChromeHost.folded === null ? undefined : codeChromeHost.folded ? 'true' : 'false'
    "
    @mousedown="onHostMouseDown"
  >
    <Teleport v-if="editor && $slots.bubble" :to="bubble">
      <slot name="bubble" :editor="editor" />
    </Teleport>
    <EditorContent :editor="editor" />
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
        :read-only="!editable"
        @remove="dropUpload(item.key)"
        @uploaded="insertUploaded(item.key, $event)"
      />
    </div>
    <slot
      v-if="editor && $slots.controls"
      name="controls"
      :editor="editor"
      :gutter="gutter"
      :editable="editable"
    />
  </div>
</template>
