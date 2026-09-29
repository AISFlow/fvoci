<script setup lang="ts">
import { formatPersonName, notificationMessage, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import {
  notificationHref,
  payloadRecord,
  type NotificationItem,
} from "@/features/notifications/notification-target";
import { api, ensureOk, loadErrorMessage } from "@/lib/api";
import type { NotificationFilter } from "@/lib/queries";
import { notificationListQuery } from "@/lib/queries";
import QueryError from "../components/QueryError.vue";
import QueryLoading from "../components/QueryLoading.vue";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import { followAppHref } from "../session/navigation";
import { useWorkspaceSession } from "../session/useWorkspaceSession";
import "@/features/notifications/notifications.css";

const TABS: { value: NotificationFilter; label: string }[] = [
  { value: "all", label: t("notif.filter.all") },
  { value: "unread", label: t("notif.filter.unread") },
  { value: "archived", label: t("notif.tab.archived") },
];

const route = useRoute();
const router = useRouter();
const queryClient = useQueryClient();
const slug = computed(() => String(route.params.slug ?? ""));
const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const workspaceId = computed(() => workspace.value?.id ?? "");
const tab = ref<NotificationFilter>("all");
const extra = ref<NotificationItem[]>([]);

const list = useQuery(() => ({
  ...notificationListQuery(workspaceId.value, tab.value),
  enabled: Boolean(workspaceId.value),
}));
const items = computed(() => [...(list.data.value?.items ?? []), ...extra.value]);

const readAll = useMutation({
  mutationFn: async () =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/notifications/read-all", {
        params: { path: { workspace_id: workspaceId.value } },
      }),
    ),
  onSuccess: async () => {
    extra.value = [];
    await queryClient.invalidateQueries({ queryKey: ["notifications", workspaceId.value] });
    await queryClient.invalidateQueries({ queryKey: ["notifications-unread", workspaceId.value] });
  },
});

function selectTab(value: NotificationFilter): void {
  tab.value = value;
  extra.value = [];
}

async function openItem(item: NotificationItem): Promise<void> {
  const id = workspaceId.value;
  if (!id) return;
  if (!item.readAt) {
    await ensureOk(
      await api.PATCH("/api/v1/workspaces/{workspace_id}/notifications/{id}", {
        params: { path: { workspace_id: id, id: item.id } },
        body: { read: true },
      }),
    );
    await queryClient.invalidateQueries({ queryKey: ["notifications", id] });
    await queryClient.invalidateQueries({ queryKey: ["notifications-unread", id] });
  }
  const href = notificationHref(slug.value, item);
  if (href) followAppHref(href, router);
}

async function toggleArchive(item: NotificationItem): Promise<void> {
  const id = workspaceId.value;
  if (!id) return;
  await ensureOk(
    await api.PATCH("/api/v1/workspaces/{workspace_id}/notifications/{id}", {
      params: { path: { workspace_id: id, id: item.id } },
      body: { archived: !item.archivedAt },
    }),
  );
  extra.value = [];
  await queryClient.invalidateQueries({ queryKey: ["notifications", id] });
  await queryClient.invalidateQueries({ queryKey: ["notifications-unread", id] });
}

async function loadMore(): Promise<void> {
  const cursor = list.data.value?.nextCursor;
  const id = workspaceId.value;
  if (!cursor || !id) return;
  const page = await ensureOk(
    await api.GET("/api/v1/workspaces/{workspace_id}/notifications", {
      params: {
        path: { workspace_id: id },
        query: { filter: tab.value, cursor },
      },
    }),
  );
  extra.value = [...extra.value, ...page.items];
}
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell v-else-if="workspace" :slug="slug" :workspace-name="workspace.name" active="notifications">
    <div class="notifications-page">
      <div class="notifications-page__head">
        <h1 class="notifications-page__title">{{ t("notif.list.title") }}</h1>
        <UButton type="button" variant="outline" color="neutral" size="sm" @click="() => void readAll.mutateAsync()">
          {{ t("notif.readAllFull") }}
        </UButton>
      </div>
      <div role="tablist" :aria-label="t('notif.filter')" class="notifications-page__tabs">
        <button
          v-for="entry in TABS"
          :key="entry.value"
          type="button"
          role="tab"
          :aria-selected="tab === entry.value"
          :class="tab === entry.value ? 'notifications-page__tab is-active' : 'notifications-page__tab'"
          @click="selectTab(entry.value)"
        >
          {{ entry.label }}
        </button>
      </div>
      <QueryLoading v-if="list.isPending.value" />
      <QueryError
        v-else-if="list.isError.value"
        :message="loadErrorMessage(list.error.value)"
        @retry="() => void list.refetch()"
      />
      <p v-else-if="items.length === 0" class="notifications-page__empty">
        {{ tab === "archived" ? t("notif.emptyArchived") : t("notif.empty") }}
      </p>
      <ul v-if="items.length > 0" class="notifications-page__list">
        <li v-for="item in items" :key="item.id" class="notifications-page__row">
          <button type="button" class="notifications-page__item" @click="() => void openItem(item)">
            <span class="notifications-page__actor">
              <span v-if="!item.readAt" class="sr-only">{{ t("notif.filter.unread") }}</span>
              {{
                item.actorGivenName
                  ? formatPersonName({ givenName: item.actorGivenName, familyName: item.actorFamilyName })
                  : t("notif.unknownActor")
              }}
            </span>
            <span class="notifications-page__message">
              {{ notificationMessage({ verb: item.verb, payload: payloadRecord(item.payload) }) }}
            </span>
          </button>
          <UButton type="button" variant="outline" color="neutral" size="sm" @click="() => void toggleArchive(item)">
            {{ item.archivedAt ? t("notif.unarchive") : t("notif.archive") }}
          </UButton>
        </li>
      </ul>
      <UButton
        v-if="list.data.value?.nextCursor && extra.length === 0"
        type="button"
        variant="outline"
        color="neutral"
        size="sm"
        @click="() => void loadMore()"
      >
        {{ t("notif.list.loadMore") }}
      </UButton>
    </div>
  </WorkspaceShell>
</template>
