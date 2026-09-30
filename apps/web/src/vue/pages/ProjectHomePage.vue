<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, watch, onScopeDispose } from "vue";
import { useRoute, useRouter } from "vue-router";
import { projectDocumentsQuery, projectQuery } from "@/features/projects/queries";
import { api, ensureOk, loadErrorMessage } from "@/lib/api";
import { projectsPath } from "@/lib/href";
import QueryError from "../components/QueryError.vue";
import QueryLoading from "../components/QueryLoading.vue";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import ProjectHomeView from "../features/projects/ProjectHomeView.vue";
import { useProjectRef } from "../session/useProjectRef";
import { useWorkspaceSession } from "../session/useWorkspaceSession";
import "@/features/projects/projects.css";

// `/w/:slug/:ref` project overview (archive / unarchive / delete, document
// list). After delete, refresh the shared list before entering it.
const route = useRoute();
const router = useRouter();
const queryClient = useQueryClient();
const slug = computed(() => String(route.params.slug ?? ""));
const refParam = computed(() => String(route.params.ref ?? ""));

const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const projectRef = useProjectRef(() => workspace.value?.id, refParam);
const listItem = projectRef.project;

const workspaceId = computed(() => workspace.value?.id ?? "");
const projectId = computed(() => listItem.value?.id ?? "");
const project = useQuery(() => projectQuery(workspaceId.value, projectId.value));
const documents = useQuery(() => projectDocumentsQuery(workspaceId.value, projectId.value));

const lifecycleError = ref<string | null>(null);
const lifetime = ref(0);
let operationVersion = 0;
watch(
  [
    workspaceId,
    slug,
    refParam,
    () => session.me.value?.userId,
    () => session.me.value?.sessionId,
    () => session.me.value?.isInstanceAdmin,
    () => workspace.value?.role,
    () => session.status.value,
  ],
  () => {
    lifetime.value++;
    lifecycleError.value = null;
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  lifetime.value++;
});
type ProjectScope = {
  workspaceId: string;
  projectId: string;
  slug: string;
  reference: string;
  lifetime: number;
  operation: number;
};
const matchesScope = (scope: ProjectScope) =>
  workspaceId.value === scope.workspaceId &&
  slug.value === scope.slug &&
  refParam.value === scope.reference &&
  lifetime.value === scope.lifetime &&
  scope.operation === operationVersion;
function currentScope(): ProjectScope {
  return {
    workspaceId: workspaceId.value,
    projectId: projectId.value,
    slug: slug.value,
    reference: refParam.value,
    lifetime: lifetime.value,
    operation: ++operationVersion,
  };
}

const lifecycle = useMutation({
  mutationFn: async ({
    action,
    scope,
  }: {
    action: "archive" | "unarchive" | "delete";
    scope: ProjectScope;
  }) => {
    const ws = scope.workspaceId;
    const id = scope.projectId;
    if (!ws || !id) throw new Error("missing project");
    const path = { workspace_id: ws, project_id: id };
    if (action === "delete") {
      return ensureOk(
        await api.DELETE("/api/v1/workspaces/{workspace_id}/projects/{project_id}", {
          params: { path },
        }),
      );
    }
    return ensureOk(
      action === "archive"
        ? await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/archive", {
            params: { path },
          })
        : await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/unarchive", {
            params: { path },
          }),
    );
  },
  onSuccess: async (_data, { action, scope }) => {
    if (matchesScope(scope)) lifecycleError.value = null;
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["projects", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["project", scope.workspaceId, scope.projectId] }),
      queryClient.invalidateQueries({ queryKey: ["trash", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["wiki-discovery", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] }),
    ]);
    if (action === "delete" && matchesScope(scope)) {
      await router.push(projectsPath(scope.slug));
    }
  },
  onError: (error: unknown, { action, scope }) => {
    if (!matchesScope(scope)) return;
    lifecycleError.value =
      action === "archive"
        ? t("project.archive.failed")
        : action === "unarchive"
          ? t("project.unarchive.failed")
          : loadErrorMessage(error);
  },
});

const createDocument = useMutation({
  mutationFn: async ({ scope, rootId }: { scope: ProjectScope; rootId: string | undefined }) => {
    const ws = scope.workspaceId;
    const id = scope.projectId;
    if (!ws || !id || !rootId) throw new Error("missing project root");
    return ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents", {
        params: {
          path: { workspace_id: ws, project_id: id },
        },
        body: {
          parentId: rootId,
          title: "새 문서",
        },
      }),
    );
  },
  onSuccess: async (_doc, { scope }) => {
    await Promise.all([
      queryClient.invalidateQueries({
        queryKey: ["project-documents", scope.workspaceId, scope.projectId],
      }),
      queryClient.invalidateQueries({ queryKey: ["projects", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["wiki-discovery", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] }),
    ]);
  },
});

const loading = computed(
  () => projectRef.projects.isLoading.value || project.isLoading.value || documents.isLoading.value,
);
const error = computed(() =>
  projectRef.projects.isError.value || project.isError.value || documents.isError.value
    ? loadErrorMessage(
        projectRef.projects.error.value ?? project.error.value ?? documents.error.value,
      )
    : null,
);

async function retry(): Promise<void> {
  await Promise.all([projectRef.retry(), project.refetch(), documents.refetch()]);
}
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{
    t("load.loading")
  }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell
    v-else-if="workspace"
    :slug="slug"
    :workspace-id="workspace.id"
    :workspace-name="workspace.name"
  >
    <p v-if="projectRef.notFound.value" role="alert" class="task-form__alert">{{
      t("project.notFound")
    }}</p>
    <QueryError
      v-else-if="projectRef.failed.value"
      :message="loadErrorMessage(projectRef.projects.error.value)"
      @retry="projectRef.retry()"
    />
    <ProjectHomeView
      v-else-if="project.data.value"
      :slug="slug"
      :project="project.data.value"
      :nodes="documents.data.value?.items ?? []"
      :loading="loading"
      :error="error"
      :creating="
        createDocument.isPending.value &&
        Boolean(
          createDocument.variables.value && matchesScope(createDocument.variables.value.scope),
        )
      "
      :can-manage="listItem?.canManage ?? false"
      :lifecycle-pending="
        lifecycle.isPending.value &&
        Boolean(lifecycle.variables.value && matchesScope(lifecycle.variables.value.scope))
      "
      :lifecycle-error="lifecycleError"
      @retry="retry"
      @create-document="
        createDocument.mutate({
          scope: currentScope(),
          rootId: project.data.value?.rootDocumentId ?? undefined,
        })
      "
      @lifecycle="lifecycle.mutate({ action: $event, scope: currentScope() })"
    />
    <QueryLoading v-else-if="loading" />
    <p v-else role="alert" class="task-form__alert">{{ error ?? t("error.resource.notFound") }}</p>
  </WorkspaceShell>
</template>
