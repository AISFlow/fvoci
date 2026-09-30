<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, nextTick, ref, useId, useTemplateRef, watch, type ComponentPublicInstance } from "vue";
import NativeModal from "./NativeModal.vue";
import "@/features/projects/projects.css";

// The React ConfirmActionButton: an outline (or solid) trigger opens a modal
// <dialog>; Escape and Cancel close it; Confirm runs `onConfirm` and closes.
const props = withDefaults(
  defineProps<{
    title: string;
    description: string;
    actionLabel: string;
    disabled?: boolean;
    destructive?: boolean;
    triggerVariant?: "default" | "outline" | "destructive";
    onConfirm: () => Promise<void> | void;
  }>(),
  { disabled: false, destructive: true, triggerVariant: "outline" },
);

const open = ref(false);
const busy = ref(false);
const titleId = useId();
const confirmBtn = useTemplateRef<ComponentPublicInstance>("confirmBtn");

const triggerColor = computed(() =>
  props.triggerVariant === "destructive" ? "error" : props.triggerVariant === "outline" ? "neutral" : "primary",
);
const triggerVariant = computed(() => (props.triggerVariant === "outline" ? "outline" : "solid"));

watch(open, async (isOpen) => {
  if (!isOpen) return;
  await nextTick();
  const el = confirmBtn.value?.$el;
  if (el instanceof HTMLButtonElement) el.focus();
  else if (el instanceof HTMLElement) el.querySelector("button")?.focus();
});

function close(): void {
  open.value = false;
}

function confirm(): void {
  if (busy.value) return;
  busy.value = true;
  void Promise.resolve(props.onConfirm()).finally(() => {
    busy.value = false;
    close();
  });
}
</script>

<template>
  <UButton
    type="button"
    size="sm"
    :variant="triggerVariant"
    :color="triggerColor"
    :disabled="disabled"
    @click="open = true"
  >
    <slot />
  </UButton>
  <NativeModal :open="open" :labelled-by="titleId" @close="close">
    <h2 :id="titleId" class="project-dialog__title">{{ title }}</h2>
    <p class="mt-2 break-keep text-sm text-muted">{{ description }}</p>
    <div class="mt-4 flex justify-end gap-2">
      <UButton type="button" size="sm" variant="outline" color="neutral" @click="close">
        {{ t("common.cancel") }}
      </UButton>
      <UButton
        ref="confirmBtn"
        type="button"
        size="sm"
        :color="destructive ? 'error' : 'primary'"
        :disabled="busy"
        @click="confirm"
      >
        {{ actionLabel }}
      </UButton>
    </div>
  </NativeModal>
</template>
