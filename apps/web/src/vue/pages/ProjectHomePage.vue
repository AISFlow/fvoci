<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { projectDocumentsQuery, projectQuery } from "@/features/projects/queries";
import { api, ensureOk, loadErrorMessage } from "@/lib/api";
import { projectsPath } from "@/lib/href";
import QueryError from "../components/QueryError.vue";
import QueryLoading from "../components/QueryLoading.vue";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import ProjectHomeView from "../features/projects/ProjectHomeView.vue";
import { leaveTo } from "../session/navigation";
import { useProjectRef } from "../session/useProjectRef";
import { useWorkspaceSession } from "../session/useWorkspaceSession";
import "@/features/projects/projects.css";

// `/w/:slug/:ref` project overview (archive / unarchive / delete, document
// list). Not live: the boundary still boots React. After delete the projects
// list is still React, so leaveTo uses location.assign unless isVueAppPath.
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

const lifecycle = useMutation({
  mutationFn: async (action: "archive" | "unarchive" | "delete") => {
    const ws = workspace.value?.id;
    const id = listItem.value?.id;
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
  onSuccess: async (_data, action) => {
    lifecycleError.value = null;
    if (action === "delete") {
      leaveTo(projectsPath(slug.value), {
        assign: (url) => window.location.assign(url),
        push: (path) => void router.push(path),
      });
      return;
    }
    await queryClient.invalidateQueries({ queryKey: ["projects", workspace.value?.id] });
    await queryClient.invalidateQueries({ queryKey: ["project", workspace.value?.id, listItem.value?.id] });
    await queryClient.invalidateQueries({ queryKey: ["trash", workspace.value?.id] });
  },
  onError: (error: unknown, action) => {
    lifecycleError.value =
      action === "archive"
        ? t("project.archive.failed")
        : action === "unarchive"
          ? t("project.unarchive.failed")
          : loadErrorMessage(error);
  },
});

const createDocument = useMutation({
  mutationFn: async () => {
    const rootId = project.data.value?.rootDocumentId;
    const ws = workspace.value?.id;
    const id = listItem.value?.id;
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
  onSuccess: async () => {
    await queryClient.invalidateQueries({
      queryKey: ["project-documents", workspace.value?.id, listItem.value?.id],
    });
  },
});

const loading = computed(
  () => projectRef.projects.isLoading.value || project.isLoading.value || documents.isLoading.value,
);
const error = computed(() =>
  projectRef.projects.isError.value || project.isError.value || documents.isError.value
    ? loadErrorMessage(projectRef.projects.error.value ?? project.error.value ?? documents.error.value)
    : null,
);

function retry(): void {
  void projectRef.retry();
  void project.refetch();
  void documents.refetch();
}
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell v-else-if="workspace" :slug="slug" :workspace-name="workspace.name">
    <p v-if="projectRef.notFound.value" role="alert" class="task-form__alert">{{ t("project.notFound") }}</p>
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
      :creating="createDocument.isPending.value"
      :can-manage="listItem?.canManage ?? false"
      :lifecycle-pending="lifecycle.isPending.value"
      :lifecycle-error="lifecycleError"
      @retry="retry"
      @create-document="createDocument.mutate()"
      @lifecycle="lifecycle.mutate($event)"
    />
    <QueryLoading v-else-if="loading" />
    <p v-else role="alert" class="task-form__alert">{{ error ?? t("error.resource.notFound") }}</p>
  </WorkspaceShell>
</template>
