<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import UPageCard from "@nuxt/ui/components/PageCard.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { projectLabelsQuery } from "@/features/tasks/queries";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { TAG_COLORS, type TagColor } from "@/lib/queries/collections";
import QueryError from "../../../components/QueryError.vue";
import QueryLoading from "../../../components/QueryLoading.vue";

const props = defineProps<{ workspaceId: string; projectId: string; canEdit: boolean }>();
const client = useQueryClient();
const labels = useQuery(() => projectLabelsQuery(props.workspaceId, props.projectId));
const name = ref("");
const color = ref<TagColor>("gray");
const error = ref<string | null>(null);
const path = () => ({ workspace_id: props.workspaceId, project_id: props.projectId });
const refresh = async () => {
  error.value = null;
  await client.invalidateQueries({ queryKey: ["labels", props.workspaceId, props.projectId] });
  await client.invalidateQueries({ queryKey: ["workspace-labels", props.workspaceId] });
};
const onError = (err: unknown) => { error.value = err instanceof ProblemError ? err.title : t("error.network"); };
const create = useMutation({
  mutationFn: async () => ensureOk(await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels", {
    params: { path: path() }, body: { name: name.value.trim(), color: color.value },
  })),
  onSuccess: async () => { name.value = ""; color.value = "gray"; await refresh(); },
  onError,
});
const remove = useMutation({
  mutationFn: async (id: string) => ensureOk(await api.DELETE("/api/v1/workspaces/{workspace_id}/projects/{project_id}/labels/{label_id}", {
    params: { path: { ...path(), label_id: id } },
  })),
  onSuccess: async () => {
    await refresh();
    await client.invalidateQueries({ queryKey: ["tasks", props.workspaceId, props.projectId] });
  },
  onError,
});
const pending = computed(() => create.isPending.value || remove.isPending.value);
function submit(): void {
  if (props.canEdit && !pending.value && name.value.trim()) create.mutate();
}
</script>

<template>
  <UPageCard as="section" variant="subtle" data-testid="project-labels-settings">
    <h2 class="settings-section__title">{{ t("project.labels") }}</h2>
    <QueryLoading v-if="labels.isPending.value" />
    <QueryError v-else-if="labels.isError.value" :message="loadErrorMessage(labels.error.value)" @retry="labels.refetch()" />
    <p v-else-if="!labels.data.value?.items.length" class="text-sm text-muted">{{ t("project.labels.empty") }}</p>
    <ul v-else class="flex flex-col gap-1">
      <li v-for="row in labels.data.value.items" :key="row.id" :data-testid="`project-label-${row.id}`" class="flex items-center justify-between gap-2">
        <span>{{ row.name }} <span class="text-muted">({{ row.color }})</span></span>
        <UButton v-if="canEdit" type="button" size="sm" variant="outline" color="neutral" :disabled="pending" @click="remove.mutate(row.id)">{{ t("project.labels.delete") }}</UButton>
      </li>
    </ul>
    <form v-if="canEdit" class="mt-3 flex flex-wrap items-end gap-2" @submit.prevent="submit">
      <UInput v-model="name" :aria-label="t('project.labels')" maxlength="100" :disabled="pending" class="min-w-48" />
      <select v-model="color" :aria-label="t('project.labels.color')" :disabled="pending" class="collection-select">
        <option v-for="value in TAG_COLORS" :key="value" :value="value">{{ value }}</option>
      </select>
      <UButton type="submit" size="sm" :disabled="pending || !name.trim()">{{ t("project.labels.add") }}</UButton>
    </form>
    <p v-if="error" role="alert" class="text-sm text-error">{{ error }}</p>
  </UPageCard>
</template>
