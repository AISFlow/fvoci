<script setup lang="ts">
import { computed, useId } from "vue";
import { RouterLink } from "vue-router";
import { isLocalAppPath as isVueAppPath } from "@/vue/route-paths";
import { itemPath } from "@/lib/href";
import { starItemDisplayId } from "@/lib/share-links";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";

type EntranceItem = {
  key: string;
  type: string;
  projectId: string | null;
  number: number;
  title: string;
  meta?: string;
};

const props = defineProps<{
  slug: string;
  heading: string;
  empty: string;
  items: readonly EntranceItem[];
  loading: boolean;
  error: string | null;
  onRetry: () => void;
  projectKeyById: ReadonlyMap<string, string>;
}>();

const headingId = useId();

function displayIdOf(item: EntranceItem): string | null {
  return starItemDisplayId(item, props.projectKeyById);
}

function hrefOf(displayId: string): string {
  return itemPath(props.slug, displayId);
}

/** Wiki documents stay in this app; tasks and project documents load the React page. */
function inApp(href: string): boolean {
  return isVueAppPath(href.split(/[?#]/, 1)[0] ?? "");
}

const rows = computed(() =>
  props.items.map((item) => {
    const displayId = displayIdOf(item);
    const href = displayId ? hrefOf(displayId) : null;
    return { item, displayId, href, vue: href !== null && inApp(href) };
  }),
);
</script>

<template>
  <section class="entrance__section" :aria-labelledby="headingId">
    <h2 :id="headingId" class="entrance__heading">{{ heading }}</h2>
    <QueryLoading v-if="loading" />
    <QueryError v-else-if="error" :message="error" @retry="onRetry" />
    <p v-else-if="items.length === 0" class="entrance__empty">{{ empty }}</p>
    <ul v-if="items.length > 0" class="entrance__list">
      <li v-for="row in rows" :key="row.item.key">
        <RouterLink v-if="row.vue && row.href" class="entrance__row no-underline" :to="row.href">
          <span class="entrance__key">{{ row.displayId ?? "—" }}</span>
          <span class="entrance__title">{{ row.item.title }}</span>
          <span class="entrance__meta">{{ row.item.meta ?? "" }}</span>
        </RouterLink>
        <a v-else-if="row.href" class="entrance__row no-underline" :href="row.href">
          <span class="entrance__key">{{ row.displayId ?? "—" }}</span>
          <span class="entrance__title">{{ row.item.title }}</span>
          <span class="entrance__meta">{{ row.item.meta ?? "" }}</span>
        </a>
        <span v-else class="entrance__row">
          <span class="entrance__key">{{ row.displayId ?? "—" }}</span>
          <span class="entrance__title">{{ row.item.title }}</span>
          <span class="entrance__meta">{{ row.item.meta ?? "" }}</span>
        </span>
      </li>
    </ul>
  </section>
</template>
