<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";

// Zoom out / level / zoom in / reset, then the viewer's own page controls (slot).
defineProps<{ zoom: number; canZoomOut: boolean; canZoomIn: boolean }>();
const emit = defineEmits<{ zoomIn: []; zoomOut: []; reset: [] }>();
</script>

<template>
  <div class="attachment-viewer__tools">
    <UButton
      size="sm"
      variant="outline"
      color="neutral"
      :aria-label="t('attachment.viewer.zoomOut')"
      :disabled="!canZoomOut"
      @click="emit('zoomOut')"
    >
      −
    </UButton>
    <p class="attachment-viewer__page-label" aria-live="polite">{{ Math.round(zoom * 100) }}%</p>
    <UButton
      size="sm"
      variant="outline"
      color="neutral"
      :aria-label="t('attachment.viewer.zoomIn')"
      :disabled="!canZoomIn"
      @click="emit('zoomIn')"
    >
      +
    </UButton>
    <UButton size="sm" variant="outline" color="neutral" @click="emit('reset')">
      {{ t("attachment.viewer.resetZoom") }}
    </UButton>
    <slot />
  </div>
</template>
