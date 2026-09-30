<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { api, ensureOk, loadErrorMessage } from "@/lib/api";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string }>();
const client = useQueryClient();
const error = ref<string | null>(null);

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

const restore = useMutation({
  mutationFn: async (projectId: string) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/restore", {
        params: { path: { workspace_id: props.workspaceId, project_id: projectId } },
      }),
    ),
  onSuccess: async () => {
    error.value = null;
    await Promise.all([
      client.invalidateQueries({ queryKey: ["projects", props.workspaceId] }),
      client.invalidateQueries({ queryKey: ["trash", props.workspaceId] }),
    ]);
  },
  onError: (err: unknown) => {
    error.value = loadErrorMessage(err);
  },
});

const items = computed(() => deleted.data.value?.items ?? []);

function restoreProject(projectId: string): void {
  if (!window.confirm(`${t("project.restore.confirm.title")}\n${t("project.restore.confirm.body")}`)) {
    return;
  }
  restore.mutate(projectId);
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
  </section>
</template>
