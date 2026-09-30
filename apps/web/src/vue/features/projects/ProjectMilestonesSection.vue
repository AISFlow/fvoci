<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { ref, useId } from "vue";
import { projectMilestonesQuery } from "@/features/tasks/queries";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import QueryError from "../../components/QueryError.vue";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string; projectId: string; canManage: boolean }>();
const client = useQueryClient();
const name = ref("");
const nameId = useId();
const error = ref<string | null>(null);
const milestones = useQuery(() => projectMilestonesQuery(props.workspaceId, props.projectId));
const path = () => ({ workspace_id: props.workspaceId, project_id: props.projectId });
const refresh = () =>
  client.invalidateQueries({ queryKey: ["milestones", props.workspaceId, props.projectId] });
const onError = (err: unknown) => {
  error.value = err instanceof ProblemError ? err.title : t("error.network");
};
const create = useMutation({
  mutationFn: async (value: string) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones", {
        params: { path: path() },
        body: { name: value },
      }),
    ),
  onSuccess: async () => {
    error.value = null;
    name.value = "";
    await refresh();
  },
  onError,
});
const rename = useMutation({
  mutationFn: async (input: { id: string; name: string }) =>
    ensureOk(
      await api.PATCH(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones/{milestone_id}",
        {
          params: { path: { ...path(), milestone_id: input.id } },
          body: { name: input.name },
        },
      ),
    ),
  onSuccess: async () => {
    error.value = null;
    await refresh();
  },
  onError,
});
const remove = useMutation({
  mutationFn: async (id: string) =>
    ensureOk(
      await api.DELETE(
        "/api/v1/workspaces/{workspace_id}/projects/{project_id}/milestones/{milestone_id}",
        {
          params: { path: { ...path(), milestone_id: id } },
        },
      ),
    ),
  onSuccess: async () => {
    error.value = null;
    await refresh();
    await client.invalidateQueries({ queryKey: ["tasks", props.workspaceId, props.projectId] });
  },
  onError,
});
function submit(): void {
  if (!props.canManage || create.isPending.value || !name.value.trim()) return;
  create.mutate(name.value.trim());
}
function onRename(event: Event, id: string, previous: string): void {
  const value = (event.target as HTMLInputElement).value.trim();
  if (!props.canManage || rename.isPending.value || !value || value === previous) return;
  rename.mutate({ id, name: value });
}
</script>

<template>
  <section class="settings-section mt-6" data-testid="project-milestones">
    <h2 class="settings-section__title">{{ t("project.milestones") }}</h2>
    <div class="flex flex-col gap-3">
      <p v-if="milestones.isPending.value" role="status">{{ t("load.loading") }}</p>
      <QueryError
        v-else-if="milestones.isError.value"
        :message="loadErrorMessage(milestones.error.value)"
        @retry="milestones.refetch()"
      />
      <p v-else-if="!milestones.data.value?.items.length" class="task-home__note">{{
        t("project.milestones.empty")
      }}</p>
      <ul v-else class="flex flex-col gap-1">
        <li
          v-for="row in milestones.data.value?.items"
          :key="row.id"
          class="flex items-center justify-between gap-2"
        >
          <input
            v-if="canManage"
            :value="row.name"
            :data-testid="`project-milestone-name-${row.id}`"
            :aria-label="t('project.milestones')"
            :disabled="rename.isPending.value"
            class="h-10 rounded border px-3"
            @blur="onRename($event, row.id, row.name)"
          />
          <span v-else :data-testid="`project-milestone-${row.id}`">{{ row.name }}</span>
          <UButton
            v-if="canManage"
            type="button"
            size="sm"
            variant="outline"
            color="neutral"
            :data-testid="`project-milestone-delete-${row.id}`"
            :disabled="remove.isPending.value"
            @click="remove.mutate(row.id)"
            >{{ t("project.milestones.delete") }}</UButton
          >
        </li>
      </ul>
      <form v-if="canManage" class="flex flex-wrap items-end gap-2" @submit.prevent="submit">
        <div class="task-form__field">
          <label :for="nameId">{{ t("project.milestones") }}</label>
          <input
            :id="nameId"
            v-model="name"
            data-testid="project-milestone-name"
            :aria-label="t('project.milestones')"
            :disabled="create.isPending.value"
            class="h-10 rounded border px-3"
          />
        </div>
        <UButton
          type="submit"
          size="sm"
          data-testid="project-milestone-add"
          :disabled="create.isPending.value"
          >{{ t("project.milestones.add") }}</UButton
        >
      </form>
      <p v-if="error" role="alert" class="task-form__alert">{{ error }}</p>
    </div>
  </section>
</template>
