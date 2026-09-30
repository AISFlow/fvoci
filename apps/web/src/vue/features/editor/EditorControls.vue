<script setup lang="ts">
import type { GutterHandle, TiptapEditor } from "@fvoci/editor/vue";
import BlockGutter from "./BlockGutter.vue";
import CodeBlockChrome from "./CodeBlockChrome.vue";
import TemplateToolbar from "./TemplateToolbar.vue";
import TableHandles from "./TableHandles.vue";
import "./editor-controls.css";

// FVOCI contextual operations, with the official toolbar groups on mobile.
// The host installs the single drag and selection plugins before creation.
defineProps<{ editor: TiptapEditor; gutter: GutterHandle; editable: boolean }>();
</script>

<template>
  <BlockGutter v-if="editable" :editor="editor" :gutter="gutter" />
  <TableHandles v-if="editable" :editor="editor" />
  <TemplateToolbar v-if="editable" :editor="editor" mode="mobile" />
  <CodeBlockChrome :editor="editor" />
</template>
