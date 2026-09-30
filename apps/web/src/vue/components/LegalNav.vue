<script setup lang="ts">
import { t } from "@fvoci/i18n";

const docs = [
  { kind: "terms", label: t("legal.terms") },
  { kind: "privacy", label: t("legal.privacy") },
] as const;
</script>

<template>
  <nav class="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted" :aria-label="t('operator.title')">
    <a href="/service-info" class="underline underline-offset-2">{{ t("operator.title") }}</a>
    <a v-for="doc in docs" :key="doc.kind" :href="`/legal/${doc.kind}`" class="underline underline-offset-2">{{
      doc.label
    }}</a>
  </nav>
</template>
