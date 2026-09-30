<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { computed } from "vue";
import { RouterLink } from "vue-router";
import { isLocalAppPath as isVueAppPath } from "@/vue/route-paths";
import { searchItemHref } from "@/features/workspace/search-target";
import SearchResultBody from "./SearchResultBody.vue";
import type { SearchHit } from "./search-hit";
import "@/features/workspace/workspace-aux.css";

const props = defineProps<{
  slug: string;
  items: readonly SearchHit[];
  labelledBy?: string;
}>();

function typeLabel(type: string): string {
  if (type === "document") return t("search.tab.document");
  if (type === "task") return t("search.tab.task");
  if (type === "attachment") return t("search.tab.attachment");
  if (type === "comment") return t("search.tab.comment");
  return type;
}

function inApp(href: string): boolean {
  return isVueAppPath(href.split(/[?#]/, 1)[0] ?? "");
}

const rows = computed(() =>
  props.items.map((item) => {
    const href = searchItemHref(props.slug, item);
    return { item, href, vue: href !== null && inApp(href), label: typeLabel(item.type) };
  }),
);
</script>

<template>
  <ul class="search-results" role="region" :aria-labelledby="labelledBy">
    <li v-for="row in rows" :key="`${row.item.type}:${row.item.id}`" class="search-results__row">
      <RouterLink v-if="row.vue && row.href" class="search-results__link" :to="row.href">
        <SearchResultBody :item="row.item" :type-label="row.label" />
      </RouterLink>
      <a v-else-if="row.href" class="search-results__link" :href="row.href">
        <SearchResultBody :item="row.item" :type-label="row.label" />
      </a>
      <div v-else class="search-results__link">
        <SearchResultBody :item="row.item" :type-label="row.label" />
      </div>
    </li>
  </ul>
</template>
