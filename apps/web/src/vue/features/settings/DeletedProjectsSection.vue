<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, onScopeDispose, ref, watch } from "vue";
import { meQuery } from "@/lib/queries";
import { api, ensureOk, loadErrorMessage } from "@/lib/api";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import ConfirmDialog from "./ConfirmDialog.vue";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string }>();
const client = useQueryClient();
const me = useQuery(meQuery);
const error = ref<string | null>(null);
const restoreTarget = ref<string | null>(null);

const deleted = useQuery(() => ({
  queryKey: ["projects", props.workspaceId, "deleted"] as const,
  queryFn: async () =>
    ensureOk(
      await api.GET("/api/v1/workspaces/{workspace_id}/projects", {
        params: { path: { workspace_id: props.workspaceId }, query: { deleted: "true" } },
      }),
    ),
  retry: false as const,
}));

type RestoreOperation = { workspaceId: string; projectId: string; lifecycle: number };
let operationLifecycle = 0;
watch([() => props.workspaceId, () => me.data.value?.userId, () => me.data.value?.sessionId],
  () => { operationLifecycle++; }, { flush: "sync" });
onScopeDispose(() => { operationLifecycle++; });
function captureOperation(projectId: string): RestoreOperation {
  return { workspaceId: props.workspaceId, projectId, lifecycle: operationLifecycle };
}
function currentOperation(operation: RestoreOperation): boolean {
  return operation.lifecycle === operationLifecycle;
}
const restore = useMutation({
  mutationFn: async (operation: RestoreOperation) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/restore", {
        params: { path: { workspace_id: operation.workspaceId, project_id: operation.projectId } },
      }),
    ),
  onSuccess: async (_result, operation) => {
    await Promise.all([
      client.invalidateQueries({ queryKey: ["projects", operation.workspaceId] }),
      client.invalidateQueries({ queryKey: ["trash", operation.workspaceId] }),
      client.invalidateQueries({ queryKey: ["wiki-discovery", operation.workspaceId] }),
      client.invalidateQueries({ queryKey: ["me", "workspaces"] }),
    ]);
    if (!currentOperation(operation)) return;
    error.value = null;
    restoreTarget.value = null;
  },
  onError: (err: unknown, operation) => {
    if (currentOperation(operation)) error.value = loadErrorMessage(err);
  },
});

const items = computed(() => deleted.data.value?.items ?? []);

function restoreProject(projectId: string): void {
  error.value = null;
  restoreTarget.value = projectId;
}
</script>

<template>
  <section class="settings-section" aria-labelledby="deleted-projects-title">
    <h2 id="deleted-projects-title" class="settings-section__title">{{ t("project.restore") }}</h2>
    <QueryLoading v-if="deleted.isLoading.value" />
    <QueryError
      v-else-if="deleted.isError.value"
      :message="loadErrorMessage(deleted.error.value)"
      @retry="deleted.refetch()"
    />
    <p v-else-if="items.length === 0" class="settings-section__lede">{{ t("project.restore.empty") }}</p>
    <ul v-if="items.length > 0" class="flex flex-col gap-2" data-testid="deleted-projects">
      <li v-for="project in items" :key="project.id" class="flex items-center justify-between gap-2">
        <span>
          <span class="font-mono">{{ project.key }}</span> {{ project.name }}
        </span>
        <UButton
          type="button"
          size="sm"
          variant="outline"
          color="neutral"
          :disabled="restore.isPending.value"
          :aria-label="`${t('trash.restore')} ${project.name}`"
          @click="restoreProject(project.id)"
        >
          {{ t("trash.restore") }}
        </UButton>
      </li>
    </ul>
    <p v-if="error" role="alert" class="text-error">{{ error }}</p>
    <ConfirmDialog
      :open="restoreTarget !== null"
      :title="t('project.restore.confirm.title')"
      :body="t('project.restore.confirm.body')"
      :action-label="t('trash.restore')"
      :pending="restore.isPending.value"
      :error="error"
      @close="restoreTarget = null"
      @confirm="restoreTarget && restore.mutate(captureOperation(restoreTarget))"
    />
  </section>
</template>
