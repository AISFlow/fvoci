<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, watchEffect } from "vue";
import { useRoute, useRouter } from "vue-router";
import { logout as logoutRequest } from "@/features/notifications/push-logout";
import { api, ensureOk, ProblemError, problemMessage } from "@/lib/api";
import { wikiPath } from "@/lib/href";
import { meQuery, setupStatusQuery, workspacesQuery } from "@/lib/queries";
import AuthenticatedLegalNav from "../features/legal/AuthenticatedLegalNav.vue";
import EmptyWorkspace from "../features/workspace/EmptyWorkspace.vue";
import WorkspaceCreateDialog from "../features/workspace/WorkspaceCreateDialog.vue";
import { loginPath, redirectTo } from "../session/navigation";
import "@/features/workspace/workspace-aux.css";
import "../features/workspace/workspace-home.css";

// The home picker preserves the installation gate before checking the session.

const route = useRoute();
const router = useRouter();
const queryClient = useQueryClient();
const createOpen = ref(false);
const logoutError = ref<string | null>(null);
const signingOut = ref(false);
const setup = useQuery(setupStatusQuery);
const ready = computed(() => setup.data.value?.needed === false && !setup.isError.value);
const me = useQuery(() => ({ ...meQuery, enabled: ready.value }));
const workspaces = useQuery(() => ({ ...workspacesQuery, enabled: ready.value }));

const denied = computed(() => route.query.denied === "workspace");
const items = computed(() => workspaces.data.value?.items ?? []);
const leaving = computed(() => setup.data.value?.needed === true || (ready.value && me.isError.value));

watchEffect(() => {
  if (!ready.value) {
    if (!setup.isError.value && setup.data.value?.needed) redirectTo("/setup");
    return;
  }
  if (!signingOut.value && me.isError.value) redirectTo(loginPath(window.location));
});

function dismissDenied(): void {
  const query = { ...route.query };
  delete query.denied;
  void router.replace({ query });
}

async function logout(): Promise<void> {
  logoutError.value = null;
  let result;
  try {
    result = await logoutRequest();
  } catch {
    logoutError.value = t("error.network");
    return;
  }
  if (!result.response.ok) {
    logoutError.value = problemMessage(new ProblemError(result.response.status), "error.auth.logout");
    return;
  }
  // Clear this app's cache without refetching the now-revoked session.
  // The explicit logout destination wins over the session-expiry redirect.
  signingOut.value = true;
  queryClient.clear();
  redirectTo("/login");
}

async function createWorkspace(input: { name: string; slug: string }): Promise<void> {
  await ensureOk(await api.POST("/api/v1/workspaces", { body: input }));
  await queryClient.invalidateQueries({ queryKey: ["me", "workspaces"] });
}
</script>

<template>
  <div v-if="setup.isError.value" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="setup.refetch()">{{ t("load.retry") }}</UButton>
  </div>
  <p v-else-if="setup.isLoading.value || workspaces.isLoading.value || me.isLoading.value || leaving" role="status" class="p-8 text-muted">
    {{ t("load.loading") }}
  </p>
  <div v-else class="app-shell">
    <div
      v-if="denied"
      role="alert"
      class="border-b border-default bg-elevated px-4 py-2 text-sm text-muted"
    >
      <span class="break-keep">{{ t("error.denied") }}</span>
      <button type="button" class="ml-3 underline underline-offset-2" @click="dismissDenied">
        {{ t("common.dismiss") }}
      </button>
    </div>
    <div
      v-if="items.length > 0 && logoutError"
      role="alert"
      class="border-b border-default bg-elevated px-4 py-2 text-sm text-muted"
    >
      {{ logoutError }}
    </div>
    <template v-if="items.length === 0">
      <main class="app-shell__main">
        <a
          v-if="me.data.value?.isInstanceAdmin"
          href="/settings/admin"
          class="text-sm underline underline-offset-2"
        >
          {{ t("admin.console") }}
        </a>
        <EmptyWorkspace
          :is-admin="me.data.value?.isInstanceAdmin === true"
          :on-create="createWorkspace"
          :on-logout="() => void logout()"
          :error="logoutError ?? (workspaces.isError.value ? t('load.listFailed') : null)"
          :on-retry="() => void workspaces.refetch()"
        />
      </main>
    </template>
    <template v-else>
      <header class="app-shell__header">
        <h1 class="text-lg font-semibold">{{ t("dashboard.title") }}</h1>
        <div class="flex gap-2">
          <a
            v-if="me.data.value?.isInstanceAdmin"
            href="/settings/admin"
            class="inline-flex h-8 items-center rounded-md border border-default px-3 text-sm font-medium hover:bg-elevated"
          >
            {{ t("admin.console") }}
          </a>
          <UButton
            v-if="me.data.value?.isInstanceAdmin"
            type="button"
            size="sm"
            @click="createOpen = true"
          >
            {{ t("workspace.create") }}
          </UButton>
          <a href="/settings/account" class="self-center text-sm underline underline-offset-2">
            {{ t("settings.account") }}
          </a>
          <UButton type="button" size="sm" variant="outline" color="neutral" @click="() => void logout()">
            {{ t("nav.logout") }}
          </UButton>
        </div>
      </header>
      <main class="app-shell__main">
        <div class="workspace-list">
          <a
            v-for="workspace in items"
            :key="workspace.id"
            class="workspace-list__item"
            :href="wikiPath(workspace.slug)"
          >
            <div>
              <strong>{{ workspace.name }}</strong>
              <p class="text-xs text-muted">{{ workspace.slug }}</p>
              <p class="text-xs text-muted">
                {{ t("dashboard.workspace.documentCount", { count: workspace.documentCount }) }}
                ·
                {{ t("dashboard.workspace.assignedCount", { count: workspace.assignedCount }) }}
              </p>
            </div>
            <span class="text-xs text-muted">{{ workspace.role }}</span>
          </a>
        </div>
      </main>
      <WorkspaceCreateDialog :open="createOpen" :on-create="createWorkspace" @close="createOpen = false" />
    </template>
    <footer class="border-t border-default px-4 py-3">
      <AuthenticatedLegalNav />
    </footer>
  </div>
</template>
