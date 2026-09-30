<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, watch } from "vue";
import { useRoute } from "vue-router";
import { projectsQuery } from "@/features/projects/queries";
import { api, ensureOk, loadErrorMessage } from "@/lib/api";
import { wikiPath } from "@/lib/href";
import { trashQuery } from "@/lib/queries/documents";
import QueryError from "../components/QueryError.vue";
import QueryLoading from "../components/QueryLoading.vue";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import { useWorkspaceSession } from "../session/useWorkspaceSession";
import "@/features/workspace/workspace-aux.css";

const route = useRoute();
const queryClient = useQueryClient();
const slug = computed(() => String(route.params.slug ?? ""));
const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const workspaceId = computed(() => workspace.value?.id ?? "");

const trash = useQuery(() => ({
  ...trashQuery(workspaceId.value),
  enabled: Boolean(workspaceId.value),
}));
const projects = useQuery(() => ({
  ...projectsQuery(workspaceId.value),
  enabled: Boolean(workspaceId.value),
}));
const projectKeys = computed(
  () => new Map((projects.data.value?.items ?? []).map((project) => [project.id, project.key] as const)),
);
const restoreError = ref<string | null>(null);
watch(workspaceId, () => { restoreError.value = null; });

const restore = useMutation({
  mutationFn: async ({ workspaceId, item }: { workspaceId: string; item: { id: string; projectId?: string | null } }) =>
    item.projectId
      ? ensureOk(
          await api.POST(
            "/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/restore",
            {
              params: {
                path: {
                  workspace_id: workspaceId,
                  project_id: item.projectId,
                  document_id: item.id,
                },
              },
            },
          ),
        )
      : ensureOk(
          await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/restore", {
            params: {
              path: { workspace_id: workspaceId, document_id: item.id },
            },
          }),
        ),
  onSuccess: async (_data, { workspaceId: id, item }) => {
    if (workspaceId.value === id) restoreError.value = null;
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["trash", id] }),
      queryClient.invalidateQueries({ queryKey: ["tree", id] }),
      queryClient.invalidateQueries({ queryKey: ["projects", id] }),
      item.projectId
        ? queryClient.invalidateQueries({
            queryKey: ["project-documents", id, item.projectId],
          })
        : Promise.resolve(),
    ]);
  },
  onError: (error: unknown, scope) => {
    if (workspaceId.value === scope.workspaceId) restoreError.value = loadErrorMessage(error);
  },
});

function onRestore(item: { id: string; projectId?: string | null }): void {
  restoreError.value = null;
  restore.mutate({ workspaceId: workspaceId.value, item });
}
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell v-else-if="workspace" :slug="slug" :workspace-id="workspace.id" :workspace-name="workspace.name" active="trash">
    <div class="trash-page">
      <div class="trash-page__head">
        <h1 class="trash-page__title">{{ t("trash.title") }}</h1>
        <a :href="wikiPath(slug)">{{ t("nav.toWiki") }}</a>
      </div>
      <p class="trash-page__note">{{ t("doc.trash.retention") }}</p>
      <p v-if="restoreError" role="alert" class="trash-page__error">{{ restoreError }}</p>
      <QueryLoading v-if="trash.isLoading.value" />
      <QueryError
        v-else-if="trash.isError.value"
        :message="loadErrorMessage(trash.error.value)"
        @retry="() => void trash.refetch()"
      />
      <p v-else-if="trash.data.value?.items.length === 0">{{ t("trash.empty") }}</p>
      <ul v-else-if="trash.data.value && trash.data.value.items.length > 0" class="trash-page__list">
        <li v-for="item in trash.data.value.items" :key="item.id" class="trash-page__row">
          <div class="trash-page__copy">
            <span class="trash-page__name">{{ item.title }}</span>
            <span v-if="item.projectId" class="trash-page__project">
              {{ projectKeys.get(item.projectId) ?? t("nav.projects") }}
            </span>
            <time class="trash-page__when" :datetime="item.deletedAt">{{
              new Date(item.deletedAt).toLocaleString()
            }}</time>
          </div>
          <UButton
            type="button"
            size="sm"
            variant="outline"
            color="neutral"
            :disabled="restore.isPending.value && restore.variables.value?.workspaceId === workspace.id"
            :aria-label="`${t('trash.restore')} ${item.title}`"
            @click="onRestore({ id: item.id, projectId: item.projectId })"
          >
            {{ t("trash.restore") }}
          </UButton>
        </li>
      </ul>
    </div>
  </WorkspaceShell>
</template>
