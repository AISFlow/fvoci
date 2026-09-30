<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed, ref, watch } from "vue";
import { useRoute, useRouter } from "vue-router";
import { api, ensureOk, loadErrorMessage } from "@/lib/api";
import { searchPath } from "@/lib/href";
import { searchQuery, type SearchTab } from "@/lib/queries";
import QueryError from "../components/QueryError.vue";
import QueryLoading from "../components/QueryLoading.vue";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import SearchResultList from "../features/search/SearchResultList.vue";
import type { SearchHit } from "../features/search/search-hit";
import { parseSearchTab, SEARCH_TABS } from "../features/search/search-tabs";
import { useWorkspaceSession } from "../session/useWorkspaceSession";

const TAB_LABEL = {
  all: "search.tab.all",
  document: "search.tab.document",
  task: "search.tab.task",
  attachment: "search.tab.attachment",
  comment: "search.tab.comment",
} as const satisfies Record<SearchTab, "search.tab.all" | "search.tab.document" | "search.tab.task" | "search.tab.attachment" | "search.tab.comment">;

const route = useRoute();
const router = useRouter();
const slug = computed(() => String(route.params.slug ?? ""));
const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const workspaceId = computed(() => workspace.value?.id ?? "");

const q = computed(() => (typeof route.query.q === "string" ? route.query.q.trim() : ""));
const tab = computed(() => parseSearchTab(route.query.tab));
const projectId = computed(() => {
  const raw = route.query.projectId;
  return typeof raw === "string" && raw.length > 0 ? raw : undefined;
});

const draft = ref(q.value);
const extra = ref<SearchHit[]>([]);
const nextCursor = ref<string | undefined>(undefined);
const loadingMore = ref(false);
const moreError = ref<string | null>(null);

const page = useQuery(() => searchQuery(workspaceId.value, q.value, tab.value, projectId.value));

watch(
  [q, tab, projectId, () => page.data.value?.nextCursor],
  () => {
    draft.value = q.value;
    extra.value = [];
    nextCursor.value = page.data.value?.nextCursor ?? undefined;
    moreError.value = null;
  },
  { immediate: true },
);

const items = computed(() => [...((page.data.value?.items ?? []) as SearchHit[]), ...extra.value]);

function replaceQuery(next: { q?: string; tab?: SearchTab; projectId?: string }): void {
  void router.replace(
    searchPath(slug.value, {
      q: next.q ?? q.value,
      tab: next.tab ?? tab.value,
      projectId: next.projectId ?? projectId.value,
    }),
  );
}

function onSubmit(event: Event): void {
  event.preventDefault();
  replaceQuery({ q: draft.value.trim() });
}

async function loadMore(): Promise<void> {
  const cursor = nextCursor.value;
  const current = workspace.value;
  if (!cursor || !current) return;
  loadingMore.value = true;
  moreError.value = null;
  try {
    const fetched = await ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/search", {
        params: {
          path: { workspace_id: current.id },
          query: {
            q: q.value,
            type: tab.value,
            cursor,
            ...(projectId.value ? { projectId: projectId.value } : {}),
          },
        },
      }),
    );
    extra.value = [...extra.value, ...((fetched.items ?? []) as SearchHit[])];
    nextCursor.value = fetched.nextCursor ?? undefined;
  } catch {
    moreError.value = t("search.loadMoreError");
  } finally {
    loadingMore.value = false;
  }
}
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell v-else-if="workspace" :slug="slug" :workspace-id="workspace.id" :workspace-name="workspace.name" active="search">
    <div class="search-page">
      <h1 id="search-page-title" class="search-page__title">{{ t("search.title") }}</h1>
      <p class="search-page__hint">{{ t("search.hint") }}</p>
      <form class="search-page__form" role="search" @submit="onSubmit">
        <label class="sr-only" for="workspace-search-q">{{ t("search.query") }}</label>
        <UInput
          id="workspace-search-q"
          v-model="draft"
          class="min-w-0 flex-1"
          :placeholder="t('search.queryPlaceholder')"
          autocomplete="off"
        />
        <UButton type="submit">{{ t("nav.search") }}</UButton>
      </form>
      <div class="search-page__tabs" role="tablist" :aria-label="t('search.resultType')">
        <button
          v-for="value in SEARCH_TABS"
          :key="value"
          type="button"
          role="tab"
          :aria-selected="tab === value"
          :class="tab === value ? 'search-page__tab is-active' : 'search-page__tab'"
          @click="replaceQuery({ tab: value })"
        >
          {{ t(TAB_LABEL[value]) }}
        </button>
      </div>
      <p v-if="!q" class="search-page__status">{{ t("search.hint") }}</p>
      <QueryLoading v-if="q && page.isLoading.value" />
      <QueryError
        v-if="q && page.isError.value"
        :message="loadErrorMessage(page.error.value)"
        @retry="() => void page.refetch()"
      />
      <p v-if="q && !page.isLoading.value && !page.isError.value && items.length === 0" class="search-page__status">
        {{ t("search.empty") }}
      </p>
      <SearchResultList v-if="items.length > 0" :slug="slug" :items="items" labelled-by="search-page-title" />
      <p v-if="moreError" role="alert" class="search-page__status">{{ moreError }}</p>
      <UButton v-if="q && nextCursor" type="button" variant="outline" color="neutral" :disabled="loadingMore" @click="loadMore">
        {{ t("search.loadMore") }}
      </UButton>
    </div>
  </WorkspaceShell>
</template>
