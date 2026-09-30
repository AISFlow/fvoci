<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed } from "vue";
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

const tree = useQuery(() => treeQuery(workspaceId.value));

const createDocument = useMutation({
  mutationFn: async () =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/documents", {
        params: { path: { workspace_id: workspace.value!.id } },
        body: { parentId: null, title: t("doc.title.untitled") },
      }),
    ),
  onSuccess: async (doc) => {
    await queryClient.invalidateQueries({ queryKey: ["tree", workspace.value?.id] });
    if (doc.displayId) {
      await router.push(documentPath(slug.value, doc.displayId));
    }
  },
});

const canCreate = computed(() => (workspace.value ? roleAtLeast(workspace.value.role, "member") : false));
const createError = computed(() =>
  createDocument.isError.value
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
      :creating="createDocument.isPending.value"
      :create-error="createError"
      :can-create="canCreate"
      :role="workspace.role"
      :on-retry="() => void tree.refetch()"
      :on-create="() => createDocument.mutate()"
    />
  </WorkspaceShell>
</template>
