<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import ViewerDownloadButton from "./ViewerDownloadButton.vue";

// A viewer that cannot show the file: the reason, a retry when fetching
// again may help, and the original download.
defineProps<{ message: string; downloadUrl: string; retryable?: boolean }>();
const emit = defineEmits<{ retry: [] }>();
</script>

<template>
  <div class="attachment-viewer__pane attachment-viewer__pane--center">
    <p role="alert" class="attachment-viewer__alert">{{ message }}</p>
    <div class="attachment-viewer__tools">
      <UButton v-if="retryable" size="sm" variant="outline" color="neutral" @click="emit('retry')">
        {{ t("load.retry") }}
      </UButton>
      <ViewerDownloadButton :href="downloadUrl" />
    </div>
  </div>
</template>
