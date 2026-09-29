<script setup lang="ts">
import type { TiptapEditor } from "@fvoci/editor/vue";
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { nextTick, ref, useTemplateRef } from "vue";

// The selection bubble: the marks the fixed toolbar does not carry and a
// link field. The field opens inline in the bubble (no portal), so the
// bubble keeps focus inside and stays open while the URL is typed.
const props = defineProps<{ editor: TiptapEditor }>();
const MARKS = ["underline", "strike", "code"] as const;
const LABEL = { underline: "U", strike: "S", code: "</>" } as const;
const linkOpen = ref(false);
const href = ref("");
const input = useTemplateRef<HTMLInputElement>("input");

function toggleLink(): void {
  const current = props.editor.getAttributes("link").href;
  href.value = typeof current === "string" ? current : "";
  linkOpen.value = !linkOpen.value;
  if (linkOpen.value) void nextTick(() => input.value?.focus());
}

function applyLink(): void {
  const next = href.value.trim();
  if (next.length === 0) return;
  props.editor.chain().focus().setLink({ href: next }).run();
  linkOpen.value = false;
}
</script>

<template>
  <div class="fvoci-ui-toolbar flex flex-wrap items-center gap-1 rounded-md border border-default bg-default p-1 shadow" role="toolbar" :aria-label="t('editor.format')">
    <UButton
      v-for="mark in MARKS"
      :key="mark"
      size="xs"
      variant="ghost"
      color="neutral"
      :aria-label="t(`editor.mark.${mark}`)"
      :aria-pressed="editor.isActive(mark)"
      @mousedown.prevent
      @click="editor.chain().focus().toggleMark(mark).run()"
    >
      {{ LABEL[mark] }}
    </UButton>
    <UButton
      size="xs"
      variant="ghost"
      color="neutral"
      :aria-pressed="editor.isActive('link')"
      :aria-expanded="linkOpen"
      @mousedown.prevent
      @click="toggleLink"
    >
      {{ t("editor.link") }}
    </UButton>
    <form v-if="linkOpen" class="flex items-center gap-1" @submit.prevent="applyLink">
      <input
        ref="input"
        v-model="href"
        type="text"
        aria-label="URL"
        placeholder="https://"
        class="h-7 w-48 rounded border border-default bg-default px-2 text-sm"
      />
      <UButton type="submit" size="xs">{{ t("editor.link.apply") }}</UButton>
    </form>
  </div>
</template>
