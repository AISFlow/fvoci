<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { LEGAL_DOCS } from "@/features/legal/legal-docs";

// The signed-in footer (features/legal/operator-info.tsx AuthenticatedLegalNav):
// service information and the policies, all React pages (full loads).
</script>

<template>
  <nav class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted" :aria-label="t('operator.title')">
    <a href="/service-info" class="underline underline-offset-2 hover:text-default">{{ t("operator.title") }}</a>
    <a
      v-for="doc in LEGAL_DOCS"
      :key="doc.kind"
      :href="`/legal/${doc.kind}`"
      class="underline underline-offset-2 hover:text-default"
      >{{ doc.label }}</a
    >
  </nav>
</template>
