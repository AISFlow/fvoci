<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { nextTick, ref, useId, useTemplateRef } from "vue";

// A button that asks before a destructive action (components/confirm-action.tsx):
// an inline alertdialog whose confirm button takes focus; Escape or cancel
// closes it and focus goes back to the trigger.
const props = withDefaults(
  defineProps<{
    title: string;
    description: string;
    actionLabel: string;
    disabled?: boolean;
    destructive?: boolean;
    /** The action; the dialog closes once it settled. */
    action: () => Promise<void> | void;
  }>(),
  { disabled: false, destructive: true },
);

const open = ref(false);
const busy = ref(false);
const titleId = useId();
const trigger = useTemplateRef<{ $el?: Element }>("trigger");
const confirmButton = useTemplateRef<{ $el?: Element }>("confirmButton");

function focus(target: { $el?: Element } | null): void {
  const element = target?.$el;
  if (element instanceof HTMLElement) element.focus();
}

async function show(): Promise<void> {
  open.value = true;
  await nextTick();
  focus(confirmButton.value);
}

function close(): void {
  open.value = false;
  focus(trigger.value);
}

function onKeydown(event: KeyboardEvent): void {
  if (event.key !== "Escape") return;
  event.preventDefault();
  close();
}

function onConfirm(): void {
  busy.value = true;
  void Promise.resolve()
    .then(() => props.action())
    .finally(() => {
      busy.value = false;
      close();
    });
}
</script>

<template>
  <UButton ref="trigger" size="sm" variant="outline" color="neutral" :disabled="props.disabled" @click="show">
    <slot />
  </UButton>
  <div
    v-if="open"
    role="alertdialog"
    aria-modal="true"
    :aria-labelledby="titleId"
    class="fixed inset-0 z-20 flex items-center justify-center bg-black/40 p-4"
    @keydown="onKeydown"
  >
    <div class="max-w-md rounded-md border border-default bg-default p-4">
      <h2 :id="titleId" class="text-xl font-semibold">{{ title }}</h2>
      <p class="mt-2 text-sm break-keep text-muted">{{ description }}</p>
      <div class="mt-4 flex justify-end gap-2">
        <UButton size="sm" variant="outline" color="neutral" @click="close">{{ t("common.cancel") }}</UButton>
        <UButton ref="confirmButton" size="sm" :color="destructive ? 'error' : 'primary'" :disabled="busy" @click="onConfirm">
          {{ actionLabel }}
        </UButton>
      </div>
    </div>
  </div>
</template>
