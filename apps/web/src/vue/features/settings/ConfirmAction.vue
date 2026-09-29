<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { ref, useId } from "vue";
import NativeModal from "../../components/NativeModal.vue";
import "@/features/projects/projects.css";

const props = withDefaults(
  defineProps<{
    title: string;
    description: string;
    actionLabel: string;
    disabled?: boolean;
    run: () => Promise<void> | void;
  }>(),
  { disabled: false },
);

const open = ref(false);
const busy = ref(false);
const titleId = useId();

function close(): void {
  open.value = false;
}

async function confirm(): Promise<void> {
  busy.value = true;
  try {
    await props.run();
  } finally {
    busy.value = false;
    close();
  }
}
</script>

<template>
  <UButton type="button" size="sm" variant="outline" color="neutral" :disabled="disabled" @click="open = true">
    <slot />
  </UButton>
  <NativeModal :open="open" :labelled-by="titleId" @close="close">
    <div class="max-w-md rounded-md border border-default bg-default p-4">
      <h2 :id="titleId" class="text-lg font-medium">{{ title }}</h2>
      <p class="mt-2 break-keep text-sm text-muted">{{ description }}</p>
      <div class="mt-4 flex justify-end gap-2">
        <UButton type="button" size="sm" variant="outline" color="neutral" @click="close">{{
          t("common.cancel")
        }}</UButton>
        <UButton type="button" size="sm" color="error" :disabled="busy" @click="confirm">{{ actionLabel }}</UButton>
      </div>
    </div>
  </NativeModal>
</template>
