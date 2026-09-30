<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, useId } from "vue";
import { projectGroupGrantsQuery } from "@/features/projects/queries";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { groupsQuery } from "@/lib/queries";
import QueryError from "../../components/QueryError.vue";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string; projectId: string; canManage: boolean }>();
const client = useQueryClient();
const error = ref<string | null>(null);
const groupId = ref("");
const role = ref("member");
const groupLabelId = useId();
const roleLabelId = useId();
const groups = useQuery(() => groupsQuery(props.workspaceId));
const grants = useQuery(() => projectGroupGrantsQuery(props.workspaceId, props.projectId));
const available = computed(() => {
  const granted = new Set(grants.data.value?.items.map(row => row.groupId));
  return groups.data.value?.items.filter(row => !granted.has(row.id)) ?? [];
});
const nameById = computed(() => new Map(groups.data.value?.items.map(row => [row.id, row.name])));
const path = () => ({ workspace_id: props.workspaceId, project_id: props.projectId });
const refresh = () => client.invalidateQueries({ queryKey: ["project-group-grants", props.workspaceId, props.projectId] });
const onError = (err: unknown) => { error.value = err instanceof ProblemError ? err.title : t("error.network"); };
const grant = useMutation({
  mutationFn: async (input: { groupId: string; role: string }) => ensureOk(await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups", {
    params: { path: path() }, body: input,
  })),
  onSuccess: async () => { error.value = null; groupId.value = ""; role.value = "member"; await refresh(); },
  onError,
});
const revoke = useMutation({
  mutationFn: async (id: string) => ensureOk(await api.DELETE("/api/v1/workspaces/{workspace_id}/projects/{project_id}/groups", {
    params: { path: path() }, body: { groupId: id },
  })),
  onSuccess: async () => { error.value = null; await refresh(); },
  onError,
});
function roleLabel(value: string): string {
  return value === "lead" ? t("projectRole.lead") : value === "member" ? t("projectRole.member") : t("projectRole.viewer");
}
function submit(): void {
  if (!props.canManage || !groupId.value || grant.isPending.value) return;
  grant.mutate({ groupId: groupId.value, role: role.value });
}
</script>

<template>
  <details class="settings-disclosure mt-6">
    <summary class="settings-disclosure__summary">{{ t("group.grant") }}</summary>
    <div class="settings-disclosure__body">
      <p v-if="grants.isPending.value" role="status">{{ t("load.loading") }}</p>
      <QueryError v-else-if="grants.isError.value || groups.isError.value" :message="loadErrorMessage(grants.error.value ?? groups.error.value)" @retry="grants.refetch(); groups.refetch()" />
      <p v-else-if="!grants.data.value?.items.length" class="text-muted">{{ groups.data.value?.items.length ? t("group.grants.empty") : t("project.groups.empty") }}</p>
      <ul v-else class="flex flex-col divide-y">
        <li v-for="row in grants.data.value?.items" :key="row.groupId" class="flex items-center justify-between gap-2 py-2">
          <span>{{ nameById.get(row.groupId) ?? row.groupId }} · {{ roleLabel(row.role) }}</span>
          <UButton v-if="canManage" type="button" size="sm" variant="outline" color="neutral" :disabled="revoke.isPending.value" @click="revoke.mutate(row.groupId)">{{ t("project.groups.remove") }}</UButton>
        </li>
      </ul>
      <form v-if="canManage && available.length" class="mt-3 flex flex-wrap items-end gap-2" @submit.prevent="submit">
        <div>
          <label :for="groupLabelId">{{ t("project.groups.label") }}</label>
          <select :id="groupLabelId" v-model="groupId" name="groupId" class="h-11 min-w-48 rounded border px-3">
            <option value="">{{ t("project.groups.label") }}</option>
            <option v-for="group in available" :key="group.id" :value="group.id">{{ group.name }}</option>
          </select>
        </div>
        <div>
          <label :for="roleLabelId">{{ t("project.groups.role") }}</label>
          <select :id="roleLabelId" v-model="role" name="role" :aria-label="t('project.groups.role')" class="h-11 min-w-28 rounded border px-3">
            <option v-for="value in ['lead', 'member', 'viewer']" :key="value" :value="value">{{ roleLabel(value) }}</option>
          </select>
        </div>
        <UButton type="submit" size="sm" :disabled="grant.isPending.value">{{ t("group.grant") }}</UButton>
      </form>
      <p v-else-if="canManage && groups.data.value?.items.length" class="text-muted">{{ t("group.grant.noneLeft") }}</p>
      <p v-if="error" role="alert" class="settings-notice settings-notice--danger">{{ error }}</p>
    </div>
  </details>
</template>
