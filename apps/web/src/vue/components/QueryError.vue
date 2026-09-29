<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";

defineProps<{ message: string }>();
const emit = defineEmits<{ retry: [] }>();
</script>

<template>
  <div role="alert" class="flex flex-wrap items-center justify-between gap-3 rounded-lg border border-error/25 bg-error/5 p-4">
    <p class="text-sm break-keep text-error">{{ message }}</p>
    <UButton size="sm" variant="outline" color="neutral" @click="emit('retry')">{{ t("load.retry") }}</UButton>
  </div>
</template>
