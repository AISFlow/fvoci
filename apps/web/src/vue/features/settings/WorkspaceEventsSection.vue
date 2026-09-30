<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useInfiniteQuery, useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { FALLBACK_TZ, formatInstant } from "@/lib/datetime";
import { loadErrorMessage } from "@/lib/api";
import { meQuery } from "@/lib/queries";
import { workspaceEventsQuery } from "@/lib/queries/workspace";
import { flattenEventPages } from "./events-pages";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string }>();
const me = useQuery(meQuery);
const events = useInfiniteQuery(() => workspaceEventsQuery(props.workspaceId));
const items = computed(() => flattenEventPages(events.data.value?.pages));
const timeZone = computed(() => me.data.value?.timezone ?? FALLBACK_TZ);
const firstPageFailed = computed(() => events.isError.value && !events.isFetchNextPageError.value);
const loadError = computed(() => (firstPageFailed.value ? loadErrorMessage(events.error.value) : null));
const loadMoreError = computed(() =>
  events.isFetchNextPageError.value ? t("settings.activity.loadMoreFailed") : null,
);

function eventTime(createdAt: string): string {
  return formatInstant(createdAt, timeZone.value, {
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
  });
}
</script>

<template>
  <section class="settings-section" aria-labelledby="workspace-events-title">
    <h2 id="workspace-events-title" class="settings-section__title">{{ t("settings.activity") }}</h2>
    <div class="flex flex-col gap-2" data-testid="workspace-events">
      <p v-if="events.isLoading.value" role="status" class="text-muted">{{ t("load.loading") }}</p>
      <div v-if="loadError" class="flex flex-col items-start gap-2">
        <p role="alert" class="text-error">{{ loadError }}</p>
        <UButton type="button" variant="outline" color="neutral" @click="events.refetch()">{{ t("load.retry") }}</UButton>
      </div>
      <p v-if="!events.isLoading.value && !loadError && items.length === 0" class="text-muted">{{ t("audit.empty") }}</p>
      <div v-if="items.length > 0" class="overflow-x-auto">
        <table class="w-full border-collapse">
          <tbody>
            <tr v-for="row in items" :key="row.id">
              <td class="border-b border-default px-2 py-2 align-top font-mono">{{ row.verb }}</td>
              <td class="border-b border-default px-2 py-2 align-top settings-tabular">{{ eventTime(row.createdAt) }}</td>
            </tr>
          </tbody>
        </table>
      </div>
      <div v-if="events.hasNextPage.value && !loadError" class="flex flex-col items-start gap-2">
        <p v-if="loadMoreError" role="alert" class="text-error">{{ loadMoreError }}</p>
        <UButton
          type="button"
          size="sm"
          variant="outline"
          color="neutral"
          :disabled="events.isFetchingNextPage.value"
          @click="events.fetchNextPage()"
        >
          {{ events.isFetchingNextPage.value ? t("load.loading") : t("task.list.loadMore") }}
        </UButton>
      </div>
    </div>
  </section>
</template>
