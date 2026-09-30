<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import UPageCard from "@nuxt/ui/components/PageCard.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { workflowQuery, type ProjectListItem } from "@/features/projects/queries";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import QueryError from "../../../components/QueryError.vue";
import QueryLoading from "../../../components/QueryLoading.vue";
import ProjectWorkflowStatus from "./ProjectWorkflowStatus.vue";
import {
  STATUS_CATEGORIES,
  categoryLabel,
  type StatusCategory,
  type StatusPatch,
} from "./workflow";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string; project: ProjectListItem }>();
const client = useQueryClient();
const workflow = useQuery(() => workflowQuery(props.workspaceId, props.project.id));
const newName = ref("");
const newCategory = ref<StatusCategory>("todo");
const error = ref<string | null>(null);
const path = () => ({
  workspace_id: props.workspaceId,
  workflow_id: workflow.data.value?.id ?? "",
});
const readOnly = computed(() => props.project.status !== "active" || !props.project.canManage);
const refresh = async () => {
  error.value = null;
  await client.invalidateQueries({ queryKey: ["workflow", props.workspaceId, props.project.id] });
  await client.invalidateQueries({ queryKey: ["workspace-statuses", props.workspaceId] });
};
const onError = (err: unknown) => {
  error.value = err instanceof ProblemError ? err.title : t("error.network");
};
const create = useMutation({
  mutationFn: async () =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses", {
        params: { path: path() },
        body: { name: newName.value.trim(), category: newCategory.value },
      }),
    ),
  onSuccess: async () => {
    newName.value = "";
    newCategory.value = "todo";
    await refresh();
  },
  onError,
});
const patch = useMutation({
  mutationFn: async (input: { id: string; patch: StatusPatch }) =>
    ensureOk(
      await api.PATCH(
        "/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses/{status_id}",
        {
          params: { path: { ...path(), status_id: input.id } },
          body: input.patch,
        },
      ),
    ),
  onSuccess: refresh,
  onError,
});
const remove = useMutation({
  mutationFn: async (id: string) =>
    ensureOk(
      await api.DELETE(
        "/api/v1/workspaces/{workspace_id}/workflows/{workflow_id}/statuses/{status_id}",
        {
          params: { path: { ...path(), status_id: id } },
        },
      ),
    ),
  onSuccess: refresh,
  onError,
});
const pending = computed(
  () => create.isPending.value || patch.isPending.value || remove.isPending.value,
);
function submit(): void {
  if (!readOnly.value && !pending.value && workflow.data.value && newName.value.trim())
    create.mutate();
}
function saveStatus(id: string, value: StatusPatch): void {
  if (!readOnly.value && !pending.value) patch.mutate({ id, patch: value });
}
function deleteStatus(id: string): void {
  if (!readOnly.value && !pending.value) remove.mutate(id);
}
</script>

<template>
  <UPageCard as="section" variant="subtle" class="mt-4" data-testid="project-workflow">
    <h2 class="settings-section__title">{{ t("project.workflow") }}</h2>
    <div class="flex flex-col gap-3">
      <QueryLoading v-if="workflow.isPending.value" />
      <QueryError
        v-else-if="workflow.isError.value"
        :message="loadErrorMessage(workflow.error.value)"
        @retry="workflow.refetch()"
      />
      <template v-else-if="workflow.data.value">
        <p v-if="workflow.data.value.statuses.length === 0" class="text-sm text-muted">{{
          t("project.workflow.empty")
        }}</p>
        <ul v-else class="flex flex-col gap-2">
          <ProjectWorkflowStatus
            v-for="row in workflow.data.value.statuses"
            :key="`${row.id}:${row.name}:${row.category}`"
            :row="row"
            :pending="pending"
            :read-only="readOnly"
            @patch="saveStatus"
            @delete="deleteStatus"
          />
        </ul>
        <form v-if="!readOnly" class="flex flex-wrap items-end gap-2" @submit.prevent="submit">
          <UInput
            v-model="newName"
            :aria-label="t('project.workflow')"
            maxlength="100"
            :disabled="pending"
            class="min-w-48"
          />
          <select
            v-model="newCategory"
            :aria-label="categoryLabel(newCategory)"
            :disabled="pending"
            class="collection-select"
          >
            <option v-for="category in STATUS_CATEGORIES" :key="category" :value="category">{{
              categoryLabel(category)
            }}</option>
          </select>
          <UButton type="submit" size="sm" :disabled="pending || !newName.trim()">{{
            t("project.workflow.add")
          }}</UButton>
        </form>
      </template>
      <p v-if="error" role="alert" class="text-sm text-error">{{ error }}</p>
    </div>
  </UPageCard>
</template>
