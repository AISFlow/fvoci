<script setup lang="ts">
import { t } from "@fvoci/i18n";
import type { SearchHit } from "./search-hit";

defineProps<{
  item: SearchHit;
  typeLabel: string;
}>();
</script>

<template>
  <span class="search-results__meta">
    <span class="search-results__kind">{{ typeLabel }}</span>
    <span v-if="item.displayId" class="search-results__id">{{ item.displayId }}</span>
  </span>
  <span class="search-results__title">{{ item.title }}</span>
  <span v-if="item.snippet && item.snippet.length > 0" class="search-results__snippet">
    <template v-for="(piece, index) in item.snippet" :key="`${item.id}-${index}`">
      <mark v-if="piece.match">{{ piece.text }}</mark>
      <span v-else>{{ piece.text }}</span>
    </template>
  </span>
  <span v-if="item.extractStatus && item.extractStatus !== 'ok'" class="search-results__badge">{{
    t("search.badge.notIndexed")
  }}</span>
</template>
