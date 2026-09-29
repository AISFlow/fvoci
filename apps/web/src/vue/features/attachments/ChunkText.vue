<script setup lang="ts">
import { computed, useTemplateRef } from "vue";
import { chunkPlainText } from "@/features/attachments/chunk-plain-text";
import { useEffectAfterRender } from "../../composables/useEffectAfterRender";

// Plain text with the search chunk's span marked and scrolled into view.
const props = defineProps<{ text: string; chunk?: number | undefined }>();
const mark = useTemplateRef<HTMLElement>("mark");
const hit = computed(() => (props.chunk === undefined ? undefined : chunkPlainText(props.text)[props.chunk]));

useEffectAfterRender([() => props.text, () => props.chunk], () => {
  mark.value?.scrollIntoView({ block: "center" });
});
</script>

<!-- Whitespace inside <pre> is content: keep each <pre> on one line. -->
<template>
  <pre v-if="hit === undefined" class="attachment-viewer__text">{{ text }}</pre>
  <pre v-else class="attachment-viewer__text">{{ text.slice(0, hit.start) }}<mark ref="mark">{{ hit.text }}</mark>{{ text.slice(hit.end) }}</pre>
</template>
