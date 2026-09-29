<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { onMounted, onUnmounted, ref } from "vue";
import { onBeforeRouteLeave, onBeforeRouteUpdate } from "vue-router";
import DiscardEditsDialog from "./DiscardEditsDialog.vue";

/**
 * Source `PendingEditsGuard`, mounted only while there are unsaved edits:
 * an in-app navigation waits for the dialog, and closing or reloading the
 * tab gets the browser's own prompt.
 */
const blocked = ref(false);
let finish: ((allow: boolean) => void) | null = null;

function intercept(): Promise<boolean> {
  return new Promise((resolve) => {
    finish = (allow) => {
      blocked.value = false;
      finish = null;
      resolve(allow);
    };
    blocked.value = true;
  });
}

onBeforeRouteLeave(() => intercept());
onBeforeRouteUpdate(() => intercept());

function warn(event: BeforeUnloadEvent): void {
  event.preventDefault();
  // Chromium before 119 and Safari only prompt when returnValue is set.
  event.returnValue = "";
}

onMounted(() => window.addEventListener("beforeunload", warn));
onUnmounted(() => {
  window.removeEventListener("beforeunload", warn);
  finish?.(false);
});

function onConfirm(): void {
  finish?.(true);
}

function onCancel(): void {
  finish?.(false);
}
</script>

<template>
  <DiscardEditsDialog
    v-if="blocked"
    :action-label="t('task.body.unsaved.leave')"
    @confirm="onConfirm"
    @cancel="onCancel"
  />
</template>
