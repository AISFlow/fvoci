<script setup lang="ts">
import type { GutterHandle, TiptapEditor } from "@fvoci/editor/vue";
import BlockGutter from "./BlockGutter.vue";
import CodeBlockChrome from "./CodeBlockChrome.vue";
import MobileToolbar from "./MobileToolbar.vue";
import TableHandles from "./TableHandles.vue";
import "./editor-controls.css";

// The editing controls inside the editor host, in react/fvoci-editor.tsx's
// order: block gutter, table handles and mobile toolbar while the body is
// editable, and the code-block chrome always.
defineProps<{ editor: TiptapEditor; gutter: GutterHandle; editable: boolean }>();
</script>

<template>
  <BlockGutter v-if="editable" :editor="editor" :gutter="gutter" />
  <TableHandles v-if="editable" :editor="editor" />
  <MobileToolbar v-if="editable" :editor="editor" />
  <CodeBlockChrome :editor="editor" />
</template>
