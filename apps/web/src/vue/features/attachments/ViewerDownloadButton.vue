<script setup lang="ts">
import { t } from "@fvoci/i18n";

// The original file (features/attachments/viewer-shell.tsx ViewerDownloadButton).
defineProps<{ href: string }>();
</script>

<template>
  <a
    :href="href"
    download
    class="inline-flex h-8 items-center rounded-md border border-default bg-default px-3 text-sm font-medium hover:bg-elevated"
    >{{ t("attachment.download") }}</a
  >
</template>
