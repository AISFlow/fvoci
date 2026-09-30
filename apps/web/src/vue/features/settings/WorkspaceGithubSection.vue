<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, useId } from "vue";
import { api, ensureOk, ProblemError } from "@/lib/api";
import type { GithubIssueLinkInput } from "@/lib/contracts";
import { workspaceGithubQuery } from "@/lib/queries";
import { githubIssueLinkForm } from "@/lib/validators";
import ConfirmDialog from "./ConfirmDialog.vue";
import { parseForm } from "./form";
import "@/features/settings/settings-shell.css";

const props = defineProps<{ workspaceId: string }>();
const queryClient = useQueryClient();
const formId = useId();
const install = useQuery(() => workspaceGithubQuery(props.workspaceId));
const actionError = ref<string | null>(null);
const confirmOpen = ref(false);
const uninstallError = ref<string | null>(null);
const linkStatus = ref<string | null>(null);
const linkError = ref<string | null>(null);
const fieldError = ref<string | null>(null);
const taskId = ref("");
const repo = ref("");
const issueNumber = ref("");

const queryKey = computed(() => workspaceGithubQuery(props.workspaceId).queryKey);

function failMessage(err: unknown): string {
  return err instanceof ProblemError ? err.title : t("error.network");
}

const startInstall = useMutation({
  mutationFn: async () =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/github/install", {
        params: { path: { workspace_id: props.workspaceId } },
      }),
    ),
  onSuccess: ({ url }) => {
    window.location.assign(url);
  },
  onError: (err: unknown) => {
    actionError.value =
      err instanceof ProblemError && err.status === 400
        ? t("github.notConfigured")
        : failMessage(err);
  },
});

const uninstall = useMutation({
  mutationFn: async () =>
    ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}/github", {
        params: { path: { workspace_id: props.workspaceId } },
      }),
    ),
  onSuccess: async () => {
    await queryClient.invalidateQueries({ queryKey: queryKey.value });
  },
});

const link = useMutation({
  mutationFn: async (input: GithubIssueLinkInput) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/github/issue-links", {
        params: { path: { workspace_id: props.workspaceId } },
        body: input,
      }),
    ),
});

const installationId = computed(() => install.data.value?.installationId ?? null);
const connected = computed(() => installationId.value !== null);
const loadError = computed(() => (install.error.value ? failMessage(install.error.value) : null));
const busy = computed(
  () => install.isPending.value || startInstall.isPending.value || uninstall.isPending.value,
);

function onLink(): void {
  linkError.value = null;
  linkStatus.value = null;
  fieldError.value = null;
  const parsed = parseForm(githubIssueLinkForm, {
    taskId: taskId.value,
    repo: repo.value,
    issueNumber: issueNumber.value,
  });
  if (!parsed.ok) {
    fieldError.value = parsed.message;
    return;
  }
  void link
    .mutateAsync({
      taskId: parsed.data.taskId,
      repo: parsed.data.repo,
      issueNumber: Number(parsed.data.issueNumber),
    })
    .then(
      () => {
        taskId.value = "";
        repo.value = "";
        issueNumber.value = "";
        linkStatus.value = t("github.issue.linked");
      },
      (err: unknown) => {
        linkError.value = failMessage(err);
      },
    );
}

function confirmUninstall(): void {
  void uninstall.mutateAsync().then(
    () => {
      confirmOpen.value = false;
    },
    (err: unknown) => {
      uninstallError.value = failMessage(err);
    },
  );
}
</script>

<template>
  <details class="settings-disclosure">
    <summary class="settings-disclosure__summary">{{ t("settings.github") }}</summary>
    <div class="settings-disclosure__body">
      <p v-if="install.isPending.value" role="status">{{ t("load.loading") }}</p>
      <p v-if="loadError" role="alert" class="settings-notice settings-notice--danger">{{
        loadError
      }}</p>
      <p v-if="!install.isPending.value && !loadError" class="text-muted">
        {{ connected ? t("github.connected") : t("github.disconnected") }}
        <template v-if="connected"> ({{ installationId }})</template>
      </p>
      <UButton
        v-if="connected"
        type="button"
        size="sm"
        variant="outline"
        color="neutral"
        class="w-fit"
        :disabled="busy || Boolean(loadError)"
        @click="
          uninstallError = null;
          confirmOpen = true;
        "
      >
        {{ t("github.uninstall") }}
      </UButton>
      <UButton
        v-else
        type="button"
        size="sm"
        class="w-fit"
        :disabled="busy || Boolean(loadError)"
        @click="
          actionError = null;
          startInstall.mutate();
        "
      >
        {{ t("github.install") }}
      </UButton>
      <p v-if="actionError" role="alert" class="settings-notice settings-notice--danger">{{
        actionError
      }}</p>
      <form class="flex flex-col gap-2" novalidate @submit.prevent="onLink">
        <p class="font-medium">{{ t("github.issue.link") }}</p>
        <div class="flex flex-col gap-1.5">
          <label :for="`${formId}-task`">{{ t("github.issue.task") }}</label>
          <UInput
            :id="`${formId}-task`"
            v-model="taskId"
            autocomplete="off"
            :disabled="link.isPending.value"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <label :for="`${formId}-repo`">{{ t("github.issue.repo") }}</label>
          <UInput
            :id="`${formId}-repo`"
            v-model="repo"
            autocomplete="off"
            :disabled="link.isPending.value"
          />
        </div>
        <div class="flex flex-col gap-1.5">
          <label :for="`${formId}-number`">{{ t("github.issue.number") }}</label>
          <UInput
            :id="`${formId}-number`"
            v-model="issueNumber"
            inputmode="numeric"
            :disabled="link.isPending.value"
          />
        </div>
        <p v-if="fieldError" class="text-error" role="alert">{{ fieldError }}</p>
        <UButton type="submit" size="sm" class="w-fit" :disabled="link.isPending.value">{{
          t("github.issue.link")
        }}</UButton>
        <p v-if="linkStatus" role="status" class="settings-notice">{{ linkStatus }}</p>
        <p v-if="linkError" role="alert" class="settings-notice settings-notice--danger">{{
          linkError
        }}</p>
      </form>
    </div>
    <ConfirmDialog
      :open="confirmOpen"
      :title="t('github.uninstall.confirm.title')"
      :body="t('github.uninstall.confirm.body')"
      :action-label="t('github.uninstall')"
      :pending="uninstall.isPending.value"
      :error="uninstallError"
      @close="confirmOpen = false"
      @confirm="confirmUninstall"
    />
  </details>
</template>
