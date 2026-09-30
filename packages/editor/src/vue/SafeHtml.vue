<script setup lang="ts">
import type { SafeHtml } from "../safe-html.js";

// The Vue app's only HTML sink (the React one is react/safe-html.tsx). It
// takes SafeHtml only: KaTeX with trust:false and its style attributes
// stripped (math-ml.ts), and HTML the server sanitized.
defineProps<{ html: SafeHtml; tag?: "div" | "span" }>();
</script>

<template>
  <component :is="tag ?? 'div'" v-html="html" />
</template>
