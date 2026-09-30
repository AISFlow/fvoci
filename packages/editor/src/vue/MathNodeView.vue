<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { NodeViewWrapper, nodeViewProps } from "@tiptap/vue-3";
import { computed, nextTick, ref, useTemplateRef, watch } from "vue";
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
let draft: string | null = null;
const input = useTemplateRef<HTMLTextAreaElement>("input");

function commit(next: string): void {
  props.updateAttributes({ latex: next });
}

/** Opens the source field. The field is uncontrolled, like the React view's
 * defaultValue: its value is written once, here, and never bound, because
 * Vue re-applies a bound value on every re-render of its template, so a
 * peer's change to this node while typing would reset what was typed. */
async function open(): Promise<void> {
  const source = latex.value;
  editing.value = true;
  await nextTick();
  const field = input.value;
  if (!field) return;
  field.value = source;
  field.focus();
}

function onInput(event: Event): void {
  const value = (event.target as HTMLTextAreaElement).value;
  draft = value === latex.value ? null : value;
}

function onBlur(event: FocusEvent): void {
  draft = null;
  commit((event.target as HTMLTextAreaElement).value);
  editing.value = false;
}

// WHY: #644 리뷰 #2 — 권한이 사라지면 편집 상태도 닫는다. 닫기 전에 마지막 초안을 커밋한다
// (#658: 언마운트되는 textarea 는 blur 를 보내지 않는다).
watch(editable, (value) => {
  if (value) return;
  editing.value = false;
  const pending = draft;
  draft = null;
  if (pending !== null) commit(pending);
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
  </NodeViewWrapper>
</template>
