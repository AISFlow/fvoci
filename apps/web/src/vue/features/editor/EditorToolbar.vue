<script setup lang="ts">
import type { TiptapEditor } from "@fvoci/editor/vue";
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";

// The document editor's fixed toolbar: undo and redo (the Yjs undo manager,
// so only this editor's own changes). Formatting lives in the selection
// bubble and the mobile toolbar, block commands in the gutter's block menu
// and the slash menu, as in the React editor. Buttons keep the editor's
// selection (mousedown does not take focus).
const props = defineProps<{ editor: TiptapEditor; disabled: boolean }>();

function run(action: () => void): void {
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
  </div>
</template>
