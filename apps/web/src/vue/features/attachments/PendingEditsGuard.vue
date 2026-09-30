<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { onMounted, onUnmounted, ref } from "vue";
import { onBeforeRouteLeave, onBeforeRouteUpdate, useRouter } from "vue-router";
import { isLocalAppPath as isVueAppPath } from "@/vue/route-paths";
import DiscardEditsDialog from "./DiscardEditsDialog.vue";
import { guardedViewerLink } from "./viewer-navigation";

/**
 * Source `PendingEditsGuard`, mounted only while there are unsaved edits:
 * an in-app navigation waits for the dialog, and closing or reloading the
 * tab gets the browser's own prompt.
 */
const blocked = ref(false);
const router = useRouter();
let discardApproved = false;
let finish: ((allow: boolean) => void) | null = null;

function intercept(): Promise<boolean> {
  finish?.(false);
  return new Promise((resolve) => {
    finish = (allow) => {
      blocked.value = false;
      finish = null;
      discardApproved = allow;
      resolve(allow);
    };
    blocked.value = true;
  });
}

onBeforeRouteLeave(() => intercept());
onBeforeRouteUpdate(() => intercept());

function warn(event: BeforeUnloadEvent): void {
  if (discardApproved) return;
  event.preventDefault();
  // Chromium before 119 and Safari only prompt when returnValue is set.
  event.returnValue = "";
}

// Shell links cross the app boundary as plain anchors. While dirty, consult
// the same route guard first; after confirmation router.afterEach performs
// the full React load. Modified clicks and byte downloads stay native.
function onLink(event: MouseEvent): void {
  const anchor = event.target instanceof Element ? event.target.closest("a[href]") : null;
  if (!(anchor instanceof HTMLAnchorElement)) return;
  const path = guardedViewerLink(event, {
    href: anchor.href,
    target: anchor.target || document.querySelector("base")?.target || "",
    download: anchor.hasAttribute("download"),
  }, window.location.href);
  if (!path) return;
  event.preventDefault();
  void router.push(path);
}

const removeAfterEach = router.afterEach((to, _from, failure) => {
  // A same-app update may leave this dirty component mounted. Only the
  // confirmed full-document handoff may bypass its native unload warning.
  if (failure || isVueAppPath(to.path)) discardApproved = false;
});
onMounted(() => {
  window.addEventListener("beforeunload", warn);
  // Run after the link's handlers: a cancelled click stays cancelled, and a
  // RouterLink has already entered the route guard and prevented its default.
  document.addEventListener("click", onLink);
});
onUnmounted(() => {
  window.removeEventListener("beforeunload", warn);
  document.removeEventListener("click", onLink);
  removeAfterEach();
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
