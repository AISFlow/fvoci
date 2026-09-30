<script setup lang="ts">
import { computed } from "vue";
import { hardenShareFragmentHtml } from "./harden-share-html";

// Server `format=fragment` HTML is already sanitized. Anchors are rewritten
// before v-html so an unsafe href is never clickable. Do not import
// @fvoci/editor: the Vue barrel pulls Tiptap/Yjs into this chunk.

const props = defineProps<{ html: string }>();
const hardened = computed(() => hardenShareFragmentHtml(props.html));
</script>

<template>
  <!-- Server fragment is sanitized; hardenShareFragmentHtml removes unsafe anchors before mount. -->
  <!-- eslint-disable-next-line vue/no-v-html -->
  <div class="share-page__body" data-testid="share-body" v-html="hardened" />
</template>
