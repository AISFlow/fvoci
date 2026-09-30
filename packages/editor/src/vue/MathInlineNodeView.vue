<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { NodeViewWrapper, nodeViewProps } from "@tiptap/vue-3";
import { computed, nextTick, onBeforeUnmount, onMounted, ref, useTemplateRef, watch } from "vue";
import SafeHtml from "./SafeHtml.vue";
import { inlineMathDrafts } from "./inline-math-drafts.js";
import { useEditable } from "./use-editable.js";
import { useMathMl } from "./use-math-ml.js";

// The inline math view (react/blocks.tsx MathInlineView): one-line input,
// Enter commits; the display is a button so click, Enter and Space open it.
const props = defineProps(nodeViewProps);
const editable = useEditable(props.editor);
const latex = computed(() => (typeof props.node.attrs.latex === "string" ? props.node.attrs.latex : ""));
const render = useMathMl(latex, false);
const empty = computed(() => latex.value.trim() === "");
const drafts = inlineMathDrafts(props.editor, props.getPos);
const restored = editable.value ? drafts.read() : undefined;
const editing = ref(Boolean(restored));
const input = useTemplateRef<HTMLInputElement>("input");

function remember(): void {
  const field = input.value;
  if (!field) return;
  drafts.write({
    value: field.value,
    focused: field.ownerDocument.activeElement === field,
    start: field.selectionStart,
    end: field.selectionEnd,
    direction: field.selectionDirection,
  });
}

onBeforeUnmount(() => { if (editing.value) remember(); });
onMounted(() => {
  if (!restored) return;
  // The Vue renderer mounts in a detached wrapper; ProseMirror attaches it
  // later in the same update. Restore focus once that update has finished.
  void nextTick(() => {
    const field = input.value;
    if (!field || !editing.value || !editable.value) return;
    field.value = restored.value;
    if (restored.focused) field.focus();
    field.setSelectionRange(restored.start, restored.end, restored.direction ?? undefined);
  });
});

/** Opens the source field, uncontrolled like the block view's (MathNodeView.vue):
 * its value is written once, so a peer's change never resets what was typed. */
function open(): void {
  drafts.begin();
  const source = latex.value;
  editing.value = true;
  void nextTick(() => {
    const field = input.value;
    if (!field) return;
    field.value = source;
    field.focus();
    remember();
  });
}

function onBlur(event: FocusEvent): void {
  const sameAtom = drafts.isCurrent();
  drafts.clear();
  editing.value = false;
  if (sameAtom && props.editor.isEditable) {
    props.updateAttributes({ latex: (event.target as HTMLInputElement).value });
  }
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter") (event.target as HTMLInputElement).blur();
}

// WHY: #644 리뷰 #2 — 권한이 사라지면 편집 상태도 닫는다(블록 수식과 같은 이유).
watch(editable, (value) => {
  if (!value) {
    editing.value = false;
    drafts.clear();
  }
});

// ProseMirror may also reuse this view for a neighbouring inline atom after
// deletion. Its draft belongs to the original Yjs item, never that neighbour.
watch(() => props.node, () => {
  if (drafts.isCurrent()) return;
  editing.value = false;
  drafts.clear();
});
</script>

<template>
  <NodeViewWrapper as="span">
    <input
      v-if="editing && editable"
      ref="input"
      class="afn-math-inline-edit"
      :aria-label="t('editor.math.latex')"
      @input="remember"
      @select="remember"
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
