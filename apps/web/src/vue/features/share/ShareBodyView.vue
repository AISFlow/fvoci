<script setup lang="ts">
import { isSafeShareHref } from "@/lib/share-links";
import { useTemplateRef, watchPostEffect } from "vue";

// Server `format=fragment` HTML is already sanitized. Links are hardened
// after render (React ShareBodyView uses useLayoutEffect) so they are never
// clickable with an unsafe href. This page must not import @fvoci/editor:
// the Vue barrel pulls Tiptap/Yjs into the chunk.

const props = defineProps<{ html: string }>();
const root = useTemplateRef<HTMLDivElement>("root");

watchPostEffect(() => {
  void props.html;
  const el = root.value;
  if (!el) return;
  for (const link of el.querySelectorAll("a")) {
    const href = link.getAttribute("href");
    if (href === null || !isSafeShareHref(href)) {
      link.removeAttribute("href");
      continue;
    }
    link.setAttribute("target", "_blank");
    link.setAttribute("rel", "noopener noreferrer");
  }
});
</script>

<template>
  <div ref="root" class="share-page__body" data-testid="share-body" v-html="html" />
</template>
