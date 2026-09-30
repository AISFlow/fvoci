<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { computed } from "vue";
import { searchHitTypeLabel, searchItemHref, type SearchResult } from "@/features/workspace/search-target";
import AppLink from "../../components/AppLink.vue";

// Search result rows (features/workspace/search-results.tsx): kind, display
// id, title, the matched snippet and a note when the file was not indexed.
// A row links to its target (a wiki document in the app, anything else with
// a full load); a hit without one is shown without a link.
const props = defineProps<{ slug: string; items: readonly SearchResult[] }>();
const rows = computed(() => props.items.map((item) => ({ item, href: searchItemHref(props.slug, item) })));
</script>

<template>
  <ul class="m-0 flex list-none flex-col gap-1 p-0">
    <li v-for="{ item, href } in rows" :key="`${item.type}:${item.id}`" class="min-w-0">
      <component
        :is="href ? AppLink : 'div'"
        v-bind="href ? { to: href } : {}"
        class="flex min-w-0 flex-col gap-0.5 rounded-lg px-2 py-1.5 text-inherit no-underline"
        :class="href ? 'hover:bg-elevated focus-visible:bg-elevated' : undefined"
      >
        <span class="flex gap-2 text-xs text-muted">
          <span>{{ searchHitTypeLabel(item.type) }}</span>
          <span v-if="item.displayId">{{ item.displayId }}</span>
        </span>
        <span class="text-sm font-medium break-keep">{{ item.title }}</span>
        <span v-if="item.snippet && item.snippet.length > 0" class="text-xs text-muted">
          <template v-for="(piece, index) in item.snippet" :key="`${item.id}-${index}`">
            <mark v-if="piece.match" class="bg-primary/20 text-inherit">{{ piece.text }}</mark>
            <span v-else>{{ piece.text }}</span>
          </template>
        </span>
        <span v-if="item.extractStatus && item.extractStatus !== 'ok'" class="text-xs text-muted">{{
          t("search.badge.notIndexed")
        }}</span>
      </component>
    </li>
  </ul>
</template>
