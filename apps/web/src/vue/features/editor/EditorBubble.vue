<script setup lang="ts">
import type { TiptapEditor } from "@fvoci/editor/vue";
import FormatToolbar from "./FormatToolbar.vue";

// The selection bubble (react/fvoci-editor.tsx's BubbleMenu): the
// formatting controls for a non-empty selection on wide screens.
defineProps<{ editor: TiptapEditor }>();
</script>

<template>
  <FormatToolbar :editor="editor" />
</template>
