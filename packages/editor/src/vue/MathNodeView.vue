<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { NodeViewWrapper, nodeViewProps } from "@tiptap/vue-3";
import {
  absolutePositionToRelativePosition,
  ySyncPluginKey,
  type ProsemirrorBinding,
} from "@tiptap/y-tiptap";
import * as Y from "yjs";
import {
  computed,
  nextTick,
  onBeforeUnmount,
  ref,
  shallowRef,
  toRaw,
  useTemplateRef,
  watch,
} from "vue";
import SafeHtml from "./SafeHtml.vue";
import { useEditable } from "./use-editable.js";
import { useMathMl } from "./use-math-ml.js";

// The block math view (react/blocks.tsx MathBlockView): MathML when it
// renders, the LaTeX source while editing. A commit writes the latex
// attribute, which Yjs carries to the other editors.
const props = defineProps(nodeViewProps);
const editable = useEditable(props.editor);
const latex = computed(() =>
  typeof props.node.attrs.latex === "string" ? props.node.attrs.latex : "",
);
const render = useMathMl(latex, true);
const empty = computed(() => latex.value.trim() === "");

const editing = ref(false);
/** The typed source not committed yet; null when it equals the node's. */
const draft = ref<string | null>(null);
const input = useTemplateRef<HTMLTextAreaElement>("input");
const cancelButton = useTemplateRef<HTMLButtonElement>("cancelButton");
const composing = ref(false);
let retired = false;
let fieldLifetime = 0;
let activeField: HTMLTextAreaElement | null = null;
// Only an uninterrupted authorized field may explicitly supersede a peer's
// latex. Retained drafts reopened after retirement keep their captured epoch.
let uninterruptedField = false;
let observedId: unknown = null;
let ownerDrift = false;
const owner = shallowRef<{
  editor: typeof props.editor;
  node: typeof props.node;
  id: unknown;
  latex: string;
  native: ReturnType<typeof mathAtomAt>;
} | null>(null);
const stale = computed(() => !!owner.value && latex.value !== owner.value.latex);

/** The installed binding may rebuild an unchanged PM atom on a peer edit.
 * Observe its current native owner, using the same public position boundary
 * as inline Math; never infer ownership from its shifting position or label. */
function mathAtomAt(position: number): { doc: Y.Doc; atom: Y.XmlElement } | null {
  const sync = ySyncPluginKey.getState(props.editor.state) as
    { doc: Y.Doc; type: Y.XmlFragment; binding: ProsemirrorBinding | null } | undefined;
  if (!sync?.binding) return null;
  const boundary = absolutePositionToRelativePosition(
    position,
    sync.type,
    sync.binding.mapping,
  ) as Y.RelativePosition;
  const absolute = Y.createAbsolutePositionFromRelativePosition(boundary, sync.doc);
  if (!(absolute?.type instanceof Y.XmlFragment)) return null;
  const atom = absolute.type.get(absolute.index);
  return atom instanceof Y.XmlElement && atom.nodeName === "math" ? { doc: sync.doc, atom } : null;
}

function writable(): boolean {
  return !retired && !props.editor.isDestroyed && props.editor.isEditable;
}

function ownsField(event: Event): boolean {
  return (
    editing.value &&
    writable() &&
    editable.value &&
    activeField !== null &&
    event.target === activeField &&
    input.value === activeField
  );
}

function closeField(): void {
  uninterruptedField = false;
  fieldLifetime++;
  activeField = null;
  editing.value = false;
  composing.value = false;
}
function currentOpen(lifetime: number): boolean {
  return editing.value && lifetime === fieldLifetime;
}

function commit(next: string): boolean {
  const captured = owner.value;
  if (
    retired ||
    ownerDrift ||
    composing.value ||
    !captured ||
    captured.editor !== toRaw(props.editor) ||
    props.editor.isDestroyed ||
    !props.editor.isEditable ||
    !editable.value
  )
    return false;
  const position = props.getPos();
  if (typeof position !== "number") return false;
  const current = props.editor.state.doc.nodeAt(position);
  const native = captured.native ? mathAtomAt(position) : null;
  if (
    !current ||
    current.type !== captured.node.type ||
    (current.attrs.id !== captured.id &&
      !(
        uninterruptedField &&
        captured.native &&
        (captured.id === null || captured.id === undefined) &&
        typeof current.attrs.id === "string" &&
        current.attrs.id.length > 0 &&
        (observedId === null || observedId === undefined || current.attrs.id === observedId)
      )) ||
    (captured.native
      ? native?.doc !== captured.native.doc || native.atom !== captured.native.atom
      : current !== captured.node) ||
    (current.attrs.latex !== captured.latex && !uninterruptedField)
  )
    return false;
  if (next !== current.attrs.latex) props.updateAttributes({ latex: next });
  return true;
}

/** Opens the source field. The field is uncontrolled, like the React view's
 * defaultValue: its value is written once, here, and never bound, because
 * Vue re-applies a bound value on every re-render of its template, so a
 * peer's change to this node while typing would reset what was typed. */
async function open(): Promise<void> {
  if (!writable()) return;
  uninterruptedField = draft.value === null;
  if (draft.value === null) {
    observedId = props.node.attrs.id as unknown;
    ownerDrift = false;
    owner.value = {
      editor: toRaw(props.editor),
      // VueRenderer wraps the PM node deeply; compare its original immutable
      // identity with the raw current PM state, including idless legacy nodes.
      node: toRaw(props.node),
      id: props.node.attrs.id as unknown,
      latex: latex.value,
      native: (() => {
        const position = props.getPos();
        return typeof position === "number" ? mathAtomAt(position) : null;
      })(),
    };
  }
  const source = draft.value ?? latex.value;
  const lifetime = ++fieldLifetime;
  activeField = null;
  editing.value = true;
  await nextTick();
  const field = input.value;
  if (!field || !writable() || !currentOpen(lifetime)) return;
  activeField = field;
  field.value = source;
  field.focus();
}

function onInput(event: Event): void {
  if (!ownsField(event)) return;
  const value = (event.target as HTMLTextAreaElement).value;
  draft.value = value === latex.value ? null : value;
}

function onBlur(event: FocusEvent): void {
  if (!ownsField(event)) return;
  const value = (event.target as HTMLTextAreaElement).value;
  draft.value = value === latex.value ? null : value;
  // Keyboard focus may move to Cancel before activation. Keep its draft
  // private until that explicit action, just as pointer focus is prevented.
  if (cancelButton.value && event.relatedTarget === cancelButton.value) {
    uninterruptedField = false;
    return;
  }
  if (commit(value)) {
    draft.value = null;
    owner.value = null;
  }
  closeField();
}

function onCompositionStart(event: CompositionEvent): void {
  if (ownsField(event)) composing.value = true;
}
function onCompositionEnd(event: CompositionEvent): void {
  if (ownsField(event)) composing.value = false;
}

function cancel(): void {
  if (composing.value) return;
  draft.value = null;
  owner.value = null;
  closeField();
}

// An idless native atom may gain its first label while this field is active.
// Latch each observed label: a later observed rename/removal cannot authorize
// this old field. Unobserved intermediate history is not inferred from origin.
watch(
  () => props.node.attrs.id as unknown,
  () => {
    const captured = owner.value;
    if (
      !captured ||
      !uninterruptedField ||
      !captured.native ||
      (captured.id !== null && captured.id !== undefined)
    )
      return;
    const position = props.getPos();
    if (typeof position !== "number") return;
    const native = mathAtomAt(position);
    if (native?.doc !== captured.native.doc || native.atom !== captured.native.atom) return;
    const id: unknown = props.editor.state.doc.nodeAt(position)?.attrs.id;
    if (observedId !== null && observedId !== undefined) {
      if (id !== observedId) ownerDrift = true;
    } else if (typeof id === "string" && id.length > 0) observedId = id;
  },
  { flush: "sync" },
);

// Authorization notifications close the field without publishing a private
// draft after permission is false. The same readable view may reopen its draft;
// a peer's new latex invalidates its original owner before any later commit.
watch(
  editable,
  (value) => {
    if (value) return;
    closeField();
  },
  { flush: "sync" },
);
onBeforeUnmount(() => {
  retired = true;
  draft.value = null;
  owner.value = null;
  closeField();
});
</script>

<template>
  <NodeViewWrapper>
    <textarea
      v-if="editing && editable"
      ref="input"
      class="afn-math-edit"
      :aria-label="t('editor.math.latex')"
      @input="onInput"
      @blur="onBlur"
      @compositionstart="onCompositionStart"
      @compositionend="onCompositionEnd"
    />
    <div
      v-else-if="!editable"
      class="afn-math"
      :data-empty="empty ? 'true' : undefined"
      :data-failed="render.failed ? 'true' : undefined"
    >
      <SafeHtml v-if="render.html" :html="render.html" />
      <pre v-else>{{ empty ? t("editor.math.empty") : latex }}</pre>
    </div>
    <button
      v-else
      type="button"
      class="afn-math"
      :data-empty="empty ? 'true' : undefined"
      :data-failed="render.failed ? 'true' : undefined"
      :title="t('editor.math.edit')"
      @mousedown.prevent
      @click="open"
    >
      <SafeHtml v-if="render.html" :html="render.html" />
      <pre v-else>{{ empty ? t("editor.math.empty") : latex }}</pre>
    </button>
    <p v-if="draft !== null && stale" role="status">{{ t("editor.mode.stale") }}</p>
    <button
      v-if="draft !== null"
      ref="cancelButton"
      type="button"
      :disabled="composing"
      @pointerdown.prevent
      @mousedown.prevent
      @click="cancel"
      >{{ t("editor.mode.cancel") }}</button
    >
  </NodeViewWrapper>
</template>
