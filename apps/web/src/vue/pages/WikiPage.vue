<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, watch, onScopeDispose } from "vue";
import { useRoute, useRouter } from "vue-router";
import { projectsQuery } from "@/features/projects/queries";
import { moveDocument } from "@/features/documents/document-api";
import { resolveTreeDrop, type TreeDrop } from "../features/wiki/tree-drop";
import { documentTagPoolQuery } from "@/lib/queries/collections";
import { loadErrorMessage, api, ensureOk, ProblemError, problemMessage } from "@/lib/api";
import { documentPath } from "@/lib/href";
import { wikiDiscoveryQuery } from "@/lib/queries/documents";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import WikiHomeView from "../features/wiki/WikiHomeView.vue";
import { useWorkspaceSession } from "../session/useWorkspaceSession";

function roleAtLeast(role: string, minimum: string): boolean {
  const order = ["guest", "member", "admin", "owner"];
  return order.indexOf(role) >= order.indexOf(minimum);
}

const route = useRoute();
const router = useRouter();
const slug = computed(() => String(route.params.slug ?? ""));
const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const workspaceId = computed(() => workspace.value?.id ?? "");
const queryClient = useQueryClient();

const tag = computed(() => typeof route.query.tag === "string" && route.query.tag.length ? route.query.tag : undefined);
const lifetime = ref(0);
let createVersion = 0;
let moveVersion = 0;
watch([workspaceId, slug, tag, () => session.me.value?.userId, () => session.me.value?.sessionId,
  () => session.me.value?.isInstanceAdmin, () => workspace.value?.role, () => session.status.value],
  () => { lifetime.value++; }, { flush: "sync" });
onScopeDispose(() => { lifetime.value++; });
const currentLifetime = (scope: { workspaceId: string; lifetime: number }) =>
  scope.workspaceId === workspaceId.value && scope.lifetime === lifetime.value;
const tree = useQuery(() => wikiDiscoveryQuery(workspaceId.value, tag.value));
const projects = useQuery(() => projectsQuery(workspaceId.value));
const tags = useQuery(() => documentTagPoolQuery(workspaceId.value));
function selectTag(id?: string): void {
  void router.replace({ query: { ...route.query, tag: id }, hash: route.hash });
}
const move = useMutation({
  mutationFn: async ({ workspaceId, documentId, projectId, drop }: { workspaceId: string; documentId: string; projectId: string | null; drop: TreeDrop; lifetime: number; operation: number }) => {
    if (drop.type === "move") return moveDocument({ workspaceId, documentId, projectId }, drop.newParentId);
    return projectId ? ensureOk(await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/documents/{document_id}/sort", {
      params: { path: { workspace_id: workspaceId, project_id: projectId, document_id: documentId } }, body: { afterId: drop.afterId },
    })) : ensureOk(await api.POST("/api/v1/workspaces/{workspace_id}/documents/{document_id}/sort", {
      params: { path: { workspace_id: workspaceId, document_id: documentId } }, body: { afterId: drop.afterId },
    }));
  },
  onSuccess: async (_doc, scope) => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["tree", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["wiki-discovery", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["project-documents", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["document", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["project-document", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["projects", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] }),
    ]);
  },
});
function onDropDocument(source: string, dest: string, position: "top" | "bottom" | "onto"): void {
  if (!canCreate.value || tag.value || moving.value) return;
  const nodes = tree.data.value?.items ?? [];
  const node = nodes.find(node => node.id === source);
  const drop = resolveTreeDrop(nodes, source, dest, position);
  if (node && drop) move.mutate({ workspaceId: workspaceId.value, documentId: node.id, projectId: node.projectId, drop, lifetime: lifetime.value, operation: ++moveVersion });
}
const moving = computed(() => move.isPending.value && Boolean(move.variables.value && currentLifetime(move.variables.value) && move.variables.value.operation === moveVersion));
const moveError = computed(() => move.isError.value && Boolean(move.variables.value && currentLifetime(move.variables.value) && move.variables.value.operation === moveVersion) ? loadErrorMessage(move.error.value) : null);

const createDocument = useMutation({
  mutationFn: async (scope: { workspaceId: string; slug: string; lifetime: number; operation: number }) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/documents", {
        params: { path: { workspace_id: scope.workspaceId } },
        body: { parentId: null, title: t("doc.title.untitled") },
      }),
    ),
  onSuccess: async (doc, scope) => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["tree", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["wiki-discovery", scope.workspaceId] }),
      queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] }),
    ]);
    if (doc.displayId && currentLifetime(scope) && scope.operation === createVersion && router.currentRoute.value.params.slug === scope.slug) {
      await router.push(documentPath(scope.slug, doc.displayId));
    }
  },
});

function onCreateDocument(): void {
  const id = workspaceId.value;
  if (id) createDocument.mutate({ workspaceId: id, slug: slug.value, lifetime: lifetime.value, operation: ++createVersion });
}

const canCreate = computed(() => (workspace.value ? roleAtLeast(workspace.value.role, "member") : false));
const createError = computed(() =>
  createDocument.variables.value && currentLifetime(createDocument.variables.value) && createDocument.variables.value.operation === createVersion && createDocument.isError.value
    ? createDocument.error.value instanceof ProblemError
      ? problemMessage(createDocument.error.value, "doc.create.failed")
      : t("error.network")
    : null,
);
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell v-else-if="workspace" :slug="slug" :workspace-id="workspace.id" :workspace-name="workspace.name" active="wiki">
    <WikiHomeView
      :slug="slug"
      :nodes="tree.data.value?.items ?? []"
      :projects="projects.data.value?.items ?? []"
      :tags="tags.data.value?.items ?? []"
      :tag="tag"
      :on-select-tag="selectTag"
      :moving="moving"
      :move-error="moveError"
      @drop-document="onDropDocument"
      :loading="tree.isLoading.value || projects.isLoading.value"
      :error="tree.isError.value || projects.isError.value ? loadErrorMessage(tree.error.value ?? projects.error.value) : null"
      :creating="createDocument.isPending.value && Boolean(createDocument.variables.value && currentLifetime(createDocument.variables.value) && createDocument.variables.value.operation === createVersion)"
      :create-error="createError"
      :can-create="canCreate"
      :role="workspace.role"
      :on-retry="() => { void tree.refetch(); void projects.refetch(); }"
      :on-create="onCreateDocument"
    />
  </WorkspaceShell>
</template>
