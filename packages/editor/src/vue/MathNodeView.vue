<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { NodeViewWrapper, nodeViewProps } from "@tiptap/vue-3";
import { computed, nextTick, onBeforeUnmount, ref, shallowRef, useTemplateRef, watch } from "vue";
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
const composing = ref(false);
let retired = false;
const owner = shallowRef<{
  editor: typeof props.editor;
  node: typeof props.node;
  id: unknown;
  latex: string;
} | null>(null);
const stale = computed(() => !!owner.value && latex.value !== owner.value.latex);

function writable(): boolean {
  return !retired && !props.editor.isDestroyed && props.editor.isEditable;
}

function commit(next: string): boolean {
  const captured = owner.value;
  if (
    retired ||
    composing.value ||
    !captured ||
    captured.editor !== props.editor ||
    props.editor.isDestroyed ||
    !props.editor.isEditable ||
    !editable.value
  )
    return false;
  const position = props.getPos();
  if (typeof position !== "number") return false;
  const current = props.editor.state.doc.nodeAt(position);
  if (
    !current ||
    current.type !== captured.node.type ||
    (typeof captured.id === "string" && captured.id
      ? current.attrs.id !== captured.id
      : current !== captured.node) ||
    current.attrs.latex !== captured.latex
  )
    return false;
  if (next !== captured.latex) props.updateAttributes({ latex: next });
  return true;
}

/** Opens the source field. The field is uncontrolled, like the React view's
 * defaultValue: its value is written once, here, and never bound, because
 * Vue re-applies a bound value on every re-render of its template, so a
 * peer's change to this node while typing would reset what was typed. */
async function open(): Promise<void> {
  if (!writable()) return;
  if (draft.value === null)
    owner.value = {
      editor: props.editor,
      node: props.node,
      id: props.node.attrs.id as unknown,
      latex: latex.value,
    };
  const source = draft.value ?? latex.value;
  editing.value = true;
  await nextTick();
  const field = input.value;
  if (!field || !writable()) return;
  field.value = source;
  field.focus();
}

function onInput(event: Event): void {
  const value = (event.target as HTMLTextAreaElement).value;
  draft.value = value === latex.value ? null : value;
}

function onBlur(event: FocusEvent): void {
  const value = (event.target as HTMLTextAreaElement).value;
  draft.value = value === latex.value ? null : value;
  if (commit(value)) {
    draft.value = null;
    owner.value = null;
  }
  editing.value = false;
  composing.value = false;
}

function cancel(): void {
  if (composing.value) return;
  draft.value = null;
  owner.value = null;
  editing.value = false;
}

// Authorization notifications close the field without publishing a private
// draft after permission is false. The same readable view may reopen its draft;
// a peer's new latex invalidates its original owner before any later commit.
watch(editable, (value) => {
  if (value) return;
  editing.value = false;
  composing.value = false;
});
onBeforeUnmount(() => {
  retired = true;
  draft.value = null;
  owner.value = null;
  composing.value = false;
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
      @compositionstart="composing = true"
      @compositionend="composing = false"
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
    <button v-if="draft !== null" type="button" :disabled="composing" @click="cancel">{{
      t("editor.mode.cancel")
    }}</button>
  </NodeViewWrapper>
</template>
