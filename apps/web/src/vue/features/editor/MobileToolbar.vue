<script setup lang="ts">
import { insertSlashHere, type TiptapEditor } from "@fvoci/editor/vue";
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import FormatToolbar from "./FormatToolbar.vue";

// The bottom toolbar of narrow screens (react/mobile-toolbar.tsx): the
// formatting controls, opening upwards, and "+", which inserts a block
// through the slash menu at the caret. The stylesheet shows it only on
// narrow screens, where the selection bubble and the gutter are hidden.
defineProps<{ editor: TiptapEditor }>();
</script>

<template>
  <div class="fvoci-mobile-toolbar" data-mobile-toolbar="">
    <FormatToolbar :editor="editor" side="top" />
    <UButton variant="ghost" color="neutral" :aria-label="t('editor.mobile.insert')" @click="insertSlashHere(editor)">
      +
    </UButton>
  </div>
</template>
