<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, watch, onScopeDispose } from "vue";
import { useRoute, useRouter } from "vue-router";
import { loadErrorMessage, api, ensureOk, ProblemError, problemMessage } from "@/lib/api";
import { documentPath } from "@/lib/href";
import { treeQuery } from "@/lib/queries/documents";
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
const lifetime = ref(0);
let createVersion = 0;
watch([workspaceId, slug, () => session.me.value?.userId, () => session.me.value?.sessionId,
  () => session.me.value?.isInstanceAdmin, () => workspace.value?.role, () => session.status.value],
  () => { lifetime.value++; }, { flush: "sync" });
onScopeDispose(() => { lifetime.value++; });
const currentLifetime = (scope: { workspaceId: string; lifetime: number }) =>
  scope.workspaceId === workspaceId.value && scope.lifetime === lifetime.value;

const tree = useQuery(() => treeQuery(workspaceId.value));

const createDocument = useMutation({
  mutationFn: async (scope: { workspaceId: string; slug: string; lifetime: number; operation: number }) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/documents", {
        params: { path: { workspace_id: scope.workspaceId } },
        body: { parentId: null, title: t("doc.title.untitled") },
      }),
    ),
  onSuccess: async (doc, scope) => {
    await queryClient.invalidateQueries({ queryKey: ["tree", scope.workspaceId] });
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
      :loading="tree.isLoading.value"
      :error="tree.isError.value ? loadErrorMessage(tree.error.value) : null"
      :creating="createDocument.isPending.value && Boolean(createDocument.variables.value && currentLifetime(createDocument.variables.value) && createDocument.variables.value.operation === createVersion)"
      :create-error="createError"
      :can-create="canCreate"
      :role="workspace.role"
      :on-retry="() => void tree.refetch()"
      :on-create="onCreateDocument"
    />
  </WorkspaceShell>
</template>
