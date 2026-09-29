<script setup lang="ts">
import { nextTick, useTemplateRef, watch } from "vue";

// A modal <dialog> opened with showModal() (features/projects/native-modal.tsx):
// the browser traps focus and Escape asks to close; focus returns to the
// element that had it when the dialog opened.
const props = defineProps<{ open: boolean; labelledBy: string }>();
const emit = defineEmits<{ close: [] }>();
const dialog = useTemplateRef<HTMLDialogElement>("dialog");
let opener: HTMLElement | null = null;

watch(
  () => props.open,
  async (open) => {
    if (!open) {
      opener?.focus();
      opener = null;
      return;
    }
    if (!opener && document.activeElement instanceof HTMLElement) opener = document.activeElement;
    await nextTick();
    if (dialog.value && !dialog.value.open) dialog.value.showModal();
  },
  { immediate: true },
);

function onCancel(event: Event): void {
  event.preventDefault();
  emit("close");
}
</script>

<template>
  <dialog v-if="open" ref="dialog" class="project-dialog" :aria-labelledby="labelledBy" aria-modal="true" @cancel="onCancel">
    <slot />
  </dialog>
</template>
