<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, onScopeDispose, ref, watch, watchEffect } from "vue";
import { RouterLink, useRoute } from "vue-router";
import { showsWorkspaceSso } from "@/features/settings/workspace-sso-scope";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { documentTagsSettingsPath, templatesSettingsPath } from "@/lib/href";
import { workspaceMetaQuery } from "@/lib/queries";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import DeletedProjectsSection from "../features/settings/DeletedProjectsSection.vue";
import NotificationPrefsSection from "../features/settings/NotificationPrefsSection.vue";
import WorkspaceCalendarSection from "../features/settings/WorkspaceCalendarSection.vue";
import WorkspaceConsentsSection from "../features/settings/WorkspaceConsentsSection.vue";
import WorkspaceEventsSection from "../features/settings/WorkspaceEventsSection.vue";
import WorkspaceExportSection from "../features/settings/WorkspaceExportSection.vue";
import WorkspaceGithubSection from "../features/settings/WorkspaceGithubSection.vue";
import WorkspaceGroupsSection from "../features/settings/WorkspaceGroupsSection.vue";
import WorkspaceIdentitySection from "../features/settings/WorkspaceIdentitySection.vue";
import WorkspaceImportSection from "../features/settings/WorkspaceImportSection.vue";
import WorkspaceMembersSection from "../features/settings/WorkspaceMembersSection.vue";
import WorkspaceSsoSection from "../features/settings/WorkspaceSsoSection.vue";
import WorkspaceTokensSection from "../features/settings/WorkspaceTokensSection.vue";
import WorkspaceWebhooksSection from "../features/settings/WorkspaceWebhooksSection.vue";
import { roleAtLeast } from "../features/settings/workspace-role";
import { useWorkspaceSession } from "../session/useWorkspaceSession";
import "@/features/settings/settings-shell.css";

const route = useRoute();
const queryClient = useQueryClient();
const slug = computed(() => String(route.params.slug ?? ""));
const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const workspaceId = computed(() => workspace.value?.id ?? "");
const meta = useQuery(() => ({
  ...workspaceMetaQuery(workspaceId.value),
  enabled: Boolean(workspaceId.value),
  retry: false as const,
}));

watchEffect(() => {
  if (
    meta.error.value instanceof ProblemError &&
    (meta.error.value.status === 403 || meta.error.value.status === 404) &&
    workspace.value
  ) {
    window.location.replace("/?denied=workspace");
  }
});

const canManage = computed(() =>
  workspace.value ? roleAtLeast(workspace.value.role, "admin") : false,
);
const isOwner = computed(() => workspace.value?.role === "owner");
const memberOrAbove = computed(() =>
  workspace.value ? roleAtLeast(workspace.value.role, "member") : false,
);
const showSso = computed(() =>
  workspace.value ? showsWorkspaceSso(workspace.value.kind, canManage.value) : false,
);
const nameError = ref<string | null>(null);
const nameSaved = ref(false);
const deleteError = ref<string | null>(null);

// A new visit (including A -> B -> A), role or session owns fresh UI state.
const lifecycle = ref(0);
type WorkspaceMutation = { workspaceId: string; lifecycle: number };
const isCurrent = (input: WorkspaceMutation) =>
  input.lifecycle === lifecycle.value && input.workspaceId === workspaceId.value;
watch(
  [slug, workspaceId, () => workspace.value?.role, () => session.me.value?.userId],
  () => {
    lifecycle.value += 1;
    nameError.value = null;
    nameSaved.value = false;
    deleteError.value = null;
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  lifecycle.value += 1;
});

const rename = useMutation({
  mutationFn: async (input: WorkspaceMutation & { name: string }) =>
    ensureOk(
      await api.PATCH("/api/v1/workspaces/{workspace_id}", {
        params: { path: { workspace_id: input.workspaceId } },
        body: { name: input.name },
      }),
    ),
  onSuccess: async (_data, input) => {
    if (isCurrent(input)) {
      nameError.value = null;
      nameSaved.value = true;
    }
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] }),
      queryClient.invalidateQueries({ queryKey: ["workspaces", input.workspaceId] }),
    ]);
  },
  onError: (err: unknown, input) => {
    if (!isCurrent(input)) return;
    nameSaved.value = false;
    nameError.value = err instanceof ProblemError ? err.title : t("error.network");
  },
});

const remove = useMutation({
  mutationFn: async (input: WorkspaceMutation & { confirmSlug: string }) =>
    ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}", {
        params: { path: { workspace_id: input.workspaceId } },
        body: { confirmSlug: input.confirmSlug },
      }),
    ),
  onSuccess: (_data, input) => {
    if (isCurrent(input)) {
      deleteError.value = null;
      window.location.replace("/");
    }
    void queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] });
    void queryClient.invalidateQueries({ queryKey: ["workspaces", input.workspaceId] });
  },
  onError: (err: unknown, input) => {
    if (!isCurrent(input)) return;
    deleteError.value = err instanceof ProblemError ? err.title : t("error.network");
  },
});
const namePending = computed(
  () => rename.isPending.value && !!rename.variables.value && isCurrent(rename.variables.value),
);
const deletePending = computed(
  () => remove.isPending.value && !!remove.variables.value && isCurrent(remove.variables.value),
);

function onSaveName(name: string): void {
  if (!canManage.value || workspace.value?.kind !== "team" || namePending.value) return;
  nameError.value = null;
  nameSaved.value = false;
  rename.mutate({ name, workspaceId: workspaceId.value, lifecycle: lifecycle.value });
}

function onDelete(confirmSlug: string): void {
  if (!isOwner.value || workspace.value?.kind !== "team" || deletePending.value) return;
  deleteError.value = null;
  remove.mutate({ confirmSlug, workspaceId: workspaceId.value, lifecycle: lifecycle.value });
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
    :key="`${workspace.id}:${workspace.role}`"
    :slug="slug"
    :workspace-id="workspace.id"
    :workspace-name="workspace.name"
    active="settings"
  >
    <div v-if="meta.isError.value && !meta.data.value">
      <p role="alert" class="text-muted">{{ loadErrorMessage(meta.error.value) }}</p>
      <UButton size="sm" class="mt-2" @click="meta.refetch()">{{ t("load.retry") }}</UButton>
    </div>
    <div v-else class="settings-page">
      <nav :aria-label="t('nav.workspaceSettings')" class="mb-6 flex flex-wrap gap-3">
        <RouterLink :to="documentTagsSettingsPath(slug)" class="underline underline-offset-2">{{
          t("settings.documentTags.nav")
        }}</RouterLink>
        <RouterLink :to="templatesSettingsPath(slug)" class="underline underline-offset-2">{{
          t("settings.templates")
        }}</RouterLink>
      </nav>
      <WorkspaceIdentitySection
        :workspace-name="meta.data.value?.name ?? workspace.name"
        :workspace-slug="meta.data.value?.slug ?? workspace.slug"
        :workspace-kind="workspace.kind"
        :can-manage="canManage"
        :is-owner="isOwner"
        :name-pending="namePending"
        :name-error="nameError"
        :name-saved="nameSaved"
        :delete-pending="deletePending"
        :delete-error="deleteError"
        @save-name="onSaveName"
        @delete="onDelete"
      />
      <WorkspaceMembersSection
        v-if="memberOrAbove"
        :workspace-id="workspace.id"
        :current-user-id="session.me.value?.userId ?? null"
        :current-user-role="workspace.role"
      />
      <WorkspaceGroupsSection
        v-if="memberOrAbove"
        :workspace-id="workspace.id"
        :can-manage="canManage"
      />
      <WorkspaceConsentsSection v-if="canManage" :workspace-id="workspace.id" />
      <WorkspaceExportSection :workspace-id="workspace.id" :can-manage="canManage" />
      <WorkspaceImportSection :workspace-id="workspace.id" :can-manage="canManage" />
      <NotificationPrefsSection v-if="memberOrAbove" :workspace-id="workspace.id" />
      <WorkspaceCalendarSection :workspace-id="workspace.id" />
      <WorkspaceTokensSection v-if="canManage" :workspace-id="workspace.id" />
      <WorkspaceSsoSection v-if="showSso" :workspace-id="workspace.id" />
      <WorkspaceWebhooksSection v-if="canManage" :workspace-id="workspace.id" />
      <WorkspaceGithubSection v-if="canManage" :workspace-id="workspace.id" />
      <DeletedProjectsSection
        v-if="canManage && workspace.kind === 'team'"
        :workspace-id="workspace.id"
      />
      <WorkspaceEventsSection v-if="canManage" :workspace-id="workspace.id" />
    </div>
  </WorkspaceShell>
</template>
