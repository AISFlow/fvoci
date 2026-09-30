<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed } from "vue";
import { useRoute, useRouter } from "vue-router";
import {
  projectsQuery,
  type CloneProjectBody,
  type CreateProjectBody,
} from "@/features/projects/queries";
import { api, ensureOk, loadErrorMessage } from "@/lib/api";
import { projectTasksPath } from "@/lib/href";
import { membersQuery, meQuery } from "@/lib/queries";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import ProjectsView from "../features/projects/ProjectsView.vue";
import { useWorkspaceSession } from "../session/useWorkspaceSession";

const route = useRoute();
const router = useRouter();
const slug = computed(() => String(route.params.slug ?? ""));
const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const workspaceId = computed(() => workspace.value?.id ?? "");
const queryClient = useQueryClient();

const listQuery = useQuery(() => projectsQuery(workspaceId.value));
const members = useQuery(() => membersQuery(workspaceId.value));
const me = useQuery(meQuery);

const createProject = useMutation({
  mutationFn: async ({ workspaceId, body }: { workspaceId: string; body: CreateProjectBody }) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/projects", {
        params: { path: { workspace_id: workspaceId } },
        body,
      }),
    ),
  onSuccess: async (_data, scope) => {
    await queryClient.invalidateQueries({ queryKey: ["projects", scope.workspaceId] });
  },
});

const cloneProject = useMutation({
  mutationFn: async ({ workspaceId, projectId, body }: { workspaceId: string; projectId: string; body: CloneProjectBody }) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/clone", {
        params: { path: { workspace_id: workspaceId, project_id: projectId } },
        body,
      }),
    ),
  onSuccess: async (_data, scope) => {
    await queryClient.invalidateQueries({ queryKey: ["projects", scope.workspaceId] });
  },
});

async function onCreate(input: CreateProjectBody): Promise<void> {
  const scope = { workspaceId: workspaceId.value, slug: slug.value };
  const project = await createProject.mutateAsync({ workspaceId: scope.workspaceId, body: input });
  if (workspaceId.value === scope.workspaceId && router.currentRoute.value.params.slug === scope.slug)
    await router.push(projectTasksPath(scope.slug, project.key));
}

async function onClone(projectId: string, input: CloneProjectBody): Promise<void> {
  const scope = { workspaceId: workspaceId.value, slug: slug.value };
  const project = await cloneProject.mutateAsync({ ...scope, projectId, body: input });
  if (workspaceId.value === scope.workspaceId && router.currentRoute.value.params.slug === scope.slug)
    await router.push(projectTasksPath(scope.slug, project.key));
}
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell v-else-if="workspace" :slug="slug" :workspace-id="workspace.id" :workspace-name="workspace.name" active="projects">
    <ProjectsView
      :key="workspace.id"
      :slug="slug"
      :projects="listQuery.data.value?.items ?? []"
      :members="members.data.value?.items ?? []"
      :current-user-id="me.data.value?.userId ?? null"
      :loading="listQuery.isLoading.value"
      :error="listQuery.isError.value ? loadErrorMessage(listQuery.error.value) : null"
      :creating="createProject.isPending.value"
      :cloning="cloneProject.isPending.value"
      :on-retry="() => void listQuery.refetch()"
      :on-create="onCreate"
      :on-clone="onClone"
    />
  </WorkspaceShell>
</template>
