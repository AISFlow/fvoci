<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { NodeViewWrapper, nodeViewProps } from "@tiptap/vue-3";
import { computed, nextTick, ref, useTemplateRef, watch } from "vue";
import SafeHtml from "./SafeHtml.vue";
import { useEditable } from "./use-editable.js";
import { useMathMl } from "./use-math-ml.js";

// The inline math view (react/blocks.tsx MathInlineView): one-line input,
// Enter commits; the display is a button so click, Enter and Space open it.
const props = defineProps(nodeViewProps);
const editable = useEditable(props.editor);
const latex = computed(() => (typeof props.node.attrs.latex === "string" ? props.node.attrs.latex : ""));
const render = useMathMl(latex, false);
const empty = computed(() => latex.value.trim() === "");
const editing = ref(false);
const input = useTemplateRef<HTMLInputElement>("input");

/** Opens the source field, uncontrolled like the block view's (MathNodeView.vue):
 * its value is written once, so a peer's change never resets what was typed. */
function open(): void {
  const source = latex.value;
  editing.value = true;
  void nextTick(() => {
    const field = input.value;
    if (!field) return;
    field.value = source;
    field.focus();
  });
}

function onBlur(event: FocusEvent): void {
  props.updateAttributes({ latex: (event.target as HTMLInputElement).value });
  editing.value = false;
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter") (event.target as HTMLInputElement).blur();
}

// WHY: #644 리뷰 #2 — 권한이 사라지면 편집 상태도 닫는다(블록 수식과 같은 이유).
watch(editable, (value) => {
  if (!value) editing.value = false;
});
</script>

<template>
  <NodeViewWrapper as="span">
    <input
      v-if="editing && editable"
      ref="input"
      class="afn-math-inline-edit"
      :aria-label="t('editor.math.latex')"
      @keydown="onKeydown"
      @blur="onBlur"
    />
    <span
      v-else-if="!editable"
      class="afn-math-inline"
      :data-empty="empty ? 'true' : undefined"
      :data-failed="render.failed ? 'true' : undefined"
    >
      <SafeHtml v-if="render.html" tag="span" :html="render.html" />
      <template v-else>{{ empty ? "$…$" : latex }}</template>
    </span>
    <button
      v-else
      type="button"
      class="afn-math-inline"
      :data-empty="empty ? 'true' : undefined"
      :data-failed="render.failed ? 'true' : undefined"
      :title="t('editor.math.edit')"
      @mousedown.prevent
      @click="open"
    >
      <SafeHtml v-if="render.html" tag="span" :html="render.html" />
      <template v-else>{{ empty ? "$…$" : latex }}</template>
    </button>
  </NodeViewWrapper>
</template>
