<script setup lang="ts">
import type { TiptapEditor } from "@fvoci/editor/vue";
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";

// The document editor's fixed toolbar: undo/redo (the Yjs undo manager, so
// only this editor's own changes), block type, lists, bold and italic.
// Buttons keep the editor's selection (mousedown does not take focus).
const props = defineProps<{ editor: TiptapEditor; disabled: boolean }>();

type Action = () => void;
function run(action: Action): void {
  if (!props.disabled) action();
}
const chain = () => props.editor.chain().focus();
</script>

<template>
  <div class="fvoci-vue-toolbar mb-2 flex flex-wrap items-center gap-1" role="toolbar" :aria-label="t('editor.toolbar')">
    <UButton
      size="xs"
      variant="ghost"
      color="neutral"
      :aria-label="t('editor.undo')"
      :disabled="disabled || !editor.can().undo()"
      @mousedown.prevent
      @click="run(() => chain().undo().run())"
    >
      ↶
    </UButton>
    <UButton
      size="xs"
      variant="ghost"
      color="neutral"
      :aria-label="t('editor.redo')"
      :disabled="disabled || !editor.can().redo()"
      @mousedown.prevent
      @click="run(() => chain().redo().run())"
    >
      ↷
    </UButton>
    <span class="mx-1 h-5 w-px bg-(--ui-border)" aria-hidden="true" />
    <UButton
      size="xs"
      variant="ghost"
      color="neutral"
      :aria-pressed="editor.isActive('paragraph')"
      :disabled="disabled"
      @mousedown.prevent
      @click="run(() => chain().setParagraph().run())"
    >
      {{ t("editor.block.paragraph") }}
    </UButton>
    <UButton
      v-for="level in [1, 2, 3] as const"
      :key="level"
      size="xs"
      variant="ghost"
      color="neutral"
      :aria-label="t(`editor.block.heading${level}`)"
      :aria-pressed="editor.isActive('heading', { level })"
      :disabled="disabled"
      @mousedown.prevent
      @click="run(() => chain().toggleHeading({ level }).run())"
    >
      H{{ level }}
    </UButton>
    <span class="mx-1 h-5 w-px bg-(--ui-border)" aria-hidden="true" />
    <UButton
      size="xs"
      variant="ghost"
      color="neutral"
      :aria-pressed="editor.isActive('bulletList')"
      :disabled="disabled"
      @mousedown.prevent
      @click="run(() => chain().toggleBulletList().run())"
    >
      {{ t("editor.block.bullet") }}
    </UButton>
    <UButton
      size="xs"
      variant="ghost"
      color="neutral"
      :aria-pressed="editor.isActive('orderedList')"
      :disabled="disabled"
      @mousedown.prevent
      @click="run(() => chain().toggleOrderedList().run())"
    >
      {{ t("editor.block.ordered") }}
    </UButton>
    <UButton
      size="xs"
      variant="ghost"
      color="neutral"
      :aria-pressed="editor.isActive('taskList')"
      :disabled="disabled"
      @mousedown.prevent
      @click="run(() => chain().toggleTaskList().run())"
    >
      {{ t("editor.block.task") }}
    </UButton>
    <span class="mx-1 h-5 w-px bg-(--ui-border)" aria-hidden="true" />
    <UButton
      size="xs"
      variant="ghost"
      color="neutral"
      class="font-bold"
      :aria-label="t('editor.mark.bold')"
      :aria-pressed="editor.isActive('bold')"
      :disabled="disabled"
      @mousedown.prevent
      @click="run(() => chain().toggleBold().run())"
    >
      B
    </UButton>
    <UButton
      size="xs"
      variant="ghost"
      color="neutral"
      class="italic"
      :aria-label="t('editor.mark.italic')"
      :aria-pressed="editor.isActive('italic')"
      :disabled="disabled"
      @mousedown.prevent
      @click="run(() => chain().toggleItalic().run())"
    >
      I
    </UButton>
  </div>
</template>
