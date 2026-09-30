<script setup lang="ts">
import type { SafeHtml } from "../safe-html.js";

// This native-tag sink accepts audited SafeHtml producers: KaTeX with
// trust:false and style attributes stripped (math-ml.ts), server-rendered
// legal HTML, and fixed sandbox iframe markup from the server unfurl path.
// The brand records provenance; it does not sanitize arbitrary HTML.
defineProps<{ html: SafeHtml; tag?: "div" | "span" }>();
</script>

<template>
  <!-- This resolves only native div/span tags and accepts audited branded SafeHtml.
       Remove these exact-site exceptions when the rules recognize that native sink contract. -->
  <!-- eslint-disable-next-line vue/no-v-html, vue/no-v-text-v-html-on-component -->
  <component :is="tag ?? 'div'" v-html="html" />
</template>
