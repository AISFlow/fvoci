<script setup lang="ts">
import { nextTick, useTemplateRef, watch } from "vue";

// A modal <dialog> opened with showModal() (features/projects/native-modal.tsx):
// the browser traps focus and Escape asks to close; focus returns to the
// element that had it when the dialog opened. `dialogClass` replaces the
// project dialog look; with `closeOnBackdrop` a click on the backdrop (the
// dialog element itself, outside its content) asks to close too.
const props = withDefaults(
  defineProps<{ open: boolean; labelledBy: string; dialogClass?: string; closeOnBackdrop?: boolean }>(),
  { dialogClass: "project-dialog", closeOnBackdrop: false },
);
const emit = defineEmits<{ close: [] }>();
const dialog = useTemplateRef<HTMLDialogElement>("dialog");
let opener: HTMLElement | null = null;

watch(
  () => props.open,
  async (open) => {
    if (!open) {
      // Close it first: while the modal dialog is shown the rest of the page
      // is inert, and focusing the opener there would do nothing.
      if (dialog.value?.open) dialog.value.close();
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

function onClick(event: MouseEvent): void {
  if (props.closeOnBackdrop && event.target === dialog.value) emit("close");
}
</script>

<template>
  <dialog
    v-if="open"
    ref="dialog"
    :class="dialogClass"
    :aria-labelledby="labelledBy"
    aria-modal="true"
    @cancel="onCancel"
    @click="onClick"
  >
    <slot />
  </dialog>
</template>
