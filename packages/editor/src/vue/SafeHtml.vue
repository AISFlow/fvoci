<script setup lang="ts">
import type { SafeHtml } from "../safe-html.js";

// The Vue app's only HTML sink (the React one is react/safe-html.tsx). It
// takes SafeHtml only: KaTeX with trust:false and its style attributes
// stripped (math-ml.ts), and HTML the server sanitized.
defineProps<{ html: SafeHtml; tag?: "div" | "span" }>();
</script>

<template>
  <!-- This resolves only native div/span tags and accepts audited branded SafeHtml.
       Remove these exact-site exceptions when the rules recognize that native sink contract. -->
  <!-- eslint-disable-next-line vue/no-v-html, vue/no-v-text-v-html-on-component -->
  <component :is="tag ?? 'div'" v-html="html" />
</template>
