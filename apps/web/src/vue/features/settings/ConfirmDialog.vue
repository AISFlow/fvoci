<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useId } from "vue";
import NativeModal from "../../components/NativeModal.vue";
import "@/features/projects/projects.css";

withDefaults(
  defineProps<{
    open: boolean;
    title: string;
    body: string;
    actionLabel: string;
    pending?: boolean;
    error?: string | null;
  }>(),
  { pending: false, error: null },
);

const emit = defineEmits<{ close: []; confirm: [] }>();
const titleId = useId();
const bodyId = useId();
</script>

<template>
  <NativeModal :open="open" :labelled-by="titleId" @close="emit('close')">
    <div class="max-w-md rounded-md border border-default bg-default p-4">
      <h2 :id="titleId" class="text-lg font-medium">{{ title }}</h2>
      <p :id="bodyId" class="mt-2 break-all text-sm text-muted">{{ body }}</p>
      <p v-if="error" role="alert" class="settings-notice settings-notice--danger mt-2">{{
        error
      }}</p>
      <div class="mt-4 flex justify-end gap-2">
        <UButton type="button" size="sm" variant="outline" color="neutral" @click="emit('close')">
          {{ t("common.dismiss") }}
        </UButton>
        <UButton type="button" size="sm" :disabled="pending" @click="emit('confirm')">{{
          actionLabel
        }}</UButton>
      </div>
    </div>
  </NativeModal>
</template>
