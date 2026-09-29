<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { onMounted, useId, useTemplateRef } from "vue";

/**
 * "Leave and lose the edits?" (source `ConfirmActionDialog` with the
 * `task.body.unsaved.*` copy), in this app's alertdialog pattern.
 * Escape and 취소 keep the edits; `actionLabel` names what discarding
 * them does (default 나가기).
 */
withDefaults(defineProps<{ actionLabel?: string }>(), {
  actionLabel: t("task.body.unsaved.leave"),
});
const emit = defineEmits<{ confirm: []; cancel: [] }>();
const titleId = useId();
const cancelBtn = useTemplateRef<HTMLButtonElement>("cancel");

onMounted(() => {
  cancelBtn.value?.focus();
});

function onKeydown(event: KeyboardEvent): void {
  if (event.key === "Escape") {
    event.preventDefault();
    emit("cancel");
  }
}
</script>

<template>
  <div
    role="alertdialog"
    aria-modal="true"
    :aria-labelledby="titleId"
    class="fixed inset-0 z-20 flex items-center justify-center bg-black/40 p-4"
    @keydown="onKeydown"
  >
    <div class="max-w-md rounded-md border border-default bg-default p-4">
      <h2 :id="titleId" class="text-highlighted font-medium">{{ t("task.body.unsaved.title") }}</h2>
      <p class="mt-2 break-keep text-sm text-muted">{{ t("task.body.unsaved.body") }}</p>
      <div class="mt-4 flex justify-end gap-2">
        <UButton ref="cancel" size="sm" variant="outline" color="neutral" @click="emit('cancel')">
          {{ t("common.cancel") }}
        </UButton>
        <UButton size="sm" color="error" @click="emit('confirm')">{{ actionLabel }}</UButton>
      </div>
    </div>
  </div>
</template>
