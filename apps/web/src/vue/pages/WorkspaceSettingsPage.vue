<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, watchEffect } from "vue";
import { useRoute } from "vue-router";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { documentTagsSettingsPath, templatesSettingsPath } from "@/lib/href";
import { meQuery, workspaceMetaQuery } from "@/lib/queries";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import NotificationPrefsSection from "../features/settings/NotificationPrefsSection.vue";
import WorkspaceIdentitySection from "../features/settings/WorkspaceIdentitySection.vue";
import { roleAtLeast } from "../features/settings/workspace-role";
import { useWorkspaceSession } from "../session/useWorkspaceSession";
import "@/features/settings/settings-shell.css";

// Coordinator-owned src/app-boundary.ts still sends this path to React.
// When accepting: /^\/w\/[^/]+\/settings\/?$/i plus VUE_ROUTE_PATHS.workspaceSettings.

const route = useRoute();
const queryClient = useQueryClient();
const slug = computed(() => String(route.params.slug ?? ""));
const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const me = useQuery(meQuery);
const workspaceId = computed(() => workspace.value?.id ?? "");
const meta = useQuery(() => ({
  ...workspaceMetaQuery(workspaceId.value),
  enabled: Boolean(workspaceId.value),
  retry: false as const,
}));

watchEffect(() => {
  if (meta.isError.value && meta.data.value === undefined && workspace.value) {
    window.location.replace("/?denied=workspace");
  }
});

const canManage = computed(() => (workspace.value ? roleAtLeast(workspace.value.role, "admin") : false));
const isOwner = computed(() => workspace.value?.role === "owner");
const memberOrAbove = computed(() => (workspace.value ? roleAtLeast(workspace.value.role, "member") : false));
const nameError = ref<string | null>(null);
const nameSaved = ref(false);
const deleteError = ref<string | null>(null);

const rename = useMutation({
  mutationFn: async (name: string) =>
    ensureOk(
      await api.PATCH("/api/v1/workspaces/{workspace_id}", {
        params: { path: { workspace_id: workspaceId.value } },
        body: { name },
      }),
    ),
  onSuccess: async () => {
    nameError.value = null;
    nameSaved.value = true;
    await queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] });
    await queryClient.invalidateQueries({ queryKey: ["workspaces", workspaceId.value] });
  },
  onError: (err: unknown) => {
    nameSaved.value = false;
    nameError.value = err instanceof ProblemError ? err.title : t("error.network");
  },
});

const remove = useMutation({
  mutationFn: async (confirmSlug: string) =>
    ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}", {
        params: { path: { workspace_id: workspaceId.value } },
        body: { confirmSlug },
      }),
    ),
  onSuccess: async () => {
    deleteError.value = null;
    await queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] });
    window.location.replace("/");
  },
  onError: (err: unknown) => {
    deleteError.value = err instanceof ProblemError ? err.title : t("error.network");
  },
});

function onSaveName(name: string): void {
  nameSaved.value = false;
  rename.mutate(name);
}
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell v-else-if="workspace" :slug="slug" :workspace-id="workspace.id" :workspace-name="workspace.name" active="settings">
    <div v-if="meta.isError.value && !meta.data.value">
      <p role="alert" class="text-muted">{{ loadErrorMessage(meta.error.value) }}</p>
    </div>
    <div v-else class="settings-page">
      <nav :aria-label="t('nav.workspaceSettings')" class="mb-6 flex flex-wrap gap-3">
        <a :href="documentTagsSettingsPath(slug)" class="underline underline-offset-2">{{
          t("settings.documentTags.nav")
        }}</a>
        <a :href="templatesSettingsPath(slug)" class="underline underline-offset-2">{{ t("settings.templates") }}</a>
      </nav>
      <WorkspaceIdentitySection
        :workspace-name="meta.data.value?.name ?? workspace.name"
        :workspace-slug="meta.data.value?.slug ?? workspace.slug"
        :workspace-kind="workspace.kind"
        :can-manage="canManage"
        :is-owner="isOwner"
        :name-pending="rename.isPending.value"
        :name-error="nameError"
        :name-saved="nameSaved"
        :delete-pending="remove.isPending.value"
        :delete-error="deleteError"
        @save-name="onSaveName"
        @delete="remove.mutate($event)"
      />
      <NotificationPrefsSection v-if="memberOrAbove" :workspace-id="workspace.id" />
      <p v-if="me.data.value === undefined && me.isError.value" role="alert" class="settings-notice">
        {{ loadErrorMessage(me.error.value) }}
      </p>
    </div>
  </WorkspaceShell>
</template>
