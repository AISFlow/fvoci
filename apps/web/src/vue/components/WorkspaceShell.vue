<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { ProblemError } from "@/lib/api";
import UButton from "@nuxt/ui/components/Button.vue";
import UDashboardGroup from "@nuxt/ui/components/DashboardGroup.vue";
import UDashboardPanel from "@nuxt/ui/components/DashboardPanel.vue";
import UDashboardNavbar from "@nuxt/ui/components/DashboardNavbar.vue";
import UNavigationMenu from "@nuxt/ui/components/NavigationMenu.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed, onScopeDispose, provide, ref, watch } from "vue";
import { useRouter } from "vue-router";
import AppLink from "./AppLink.vue";
import { followAppHref, redirectTo } from "../session/navigation";
import { landingPath, type WorkspaceNav } from "@/features/workspace/workspace-nav";
import {
  myTasksPath,
  projectsPath,
  searchPath,
  settingsPath,
  wikiPath,
  workspaceHomePath,
} from "@/lib/href";
import { meQuery, workspacesQuery } from "@/lib/queries";
import { logout as logoutRequest } from "@/features/notifications/push-logout";
import {
  createSourceDraftRetirement,
  sourceDraftAuthRetiredKey,
  type SourceDraftAuthScope,
} from "../composables/useSourceDraftGuard";
import { projectsQuery } from "@/features/projects/queries";
import { useTaskStreams } from "../composables/useTaskStream";
import LegalNav from "../features/shell/LegalNav.vue";
import NotificationBell from "../features/shell/NotificationBell.vue";
import PersonalInputDialog from "../features/capture/PersonalInputDialog.vue";
import SearchPalette from "../features/shell/SearchPalette.vue";
import { useLogout } from "../features/shell/useLogout";
import { usePushSessionRebind } from "../features/shell/usePushSessionRebind";

// The Vue workspace shell: section links, workspace switch, search palette,
// notification bell, account, logout, and legal footer. Section links and
// switches use the router; logout and public legal links retain full loads.
const props = withDefaults(
  defineProps<{
    slug: string;
    workspaceId: string;
    workspaceName: string;
    /** The section the page belongs to. */
    active?: WorkspaceNav;
  }>(),
  { active: "projects" },
);

const router = useRouter();
const workspaces = useQuery(workspacesQuery);
const projects = useQuery(() => projectsQuery(props.workspaceId));
useTaskStreams(
  () => props.workspaceId,
  () => projects.data.value?.items.map((project) => project.id) ?? [],
);
const items = computed(() => workspaces.data.value?.items ?? []);
// Nuxt UI Dashboard template, fixed 57e8a76e: layouts/default.vue menu
// and pages/index.vue panel/header slots, connected to the existing FVOCI paths.
const navigation = computed(() =>
  [
    { label: t("nav.home"), to: workspaceHomePath(props.slug), active: props.active === "home" },
    { label: t("nav.wiki"), to: wikiPath(props.slug), active: props.active === "wiki" },
    { label: t("nav.projects"), to: projectsPath(props.slug), active: props.active === "projects" },
    { label: t("task.mine"), to: myTasksPath(props.slug), active: props.active === "myTasks" },
    { label: t("nav.search"), to: searchPath(props.slug), active: props.active === "search" },
    { label: t("nav.settings"), to: settingsPath(props.slug), active: props.active === "settings" },
  ].map((item) => ({ ...item, exact: true })),
);
const me = useQuery(meQuery);
const logoutLifetime = ref(0);
watch(
  [
    () => props.workspaceId,
    () => me.data.value?.userId,
    () => me.data.value?.sessionId,
    () => me.error.value instanceof ProblemError && me.error.value.status === 401,
  ],
  () => {
    logoutLifetime.value++;
  },
  { flush: "sync" },
);
onScopeDispose(() => {
  logoutLifetime.value++;
});
const retirement = createSourceDraftRetirement(() => ({
  actorId: me.data.value?.userId ?? null,
  credentialId: me.data.value?.sessionId ?? null,
  workspaceId: props.workspaceId,
  lifetime: logoutLifetime.value,
}));
provide(sourceDraftAuthRetiredKey, retirement.denied);
let logoutScope: SourceDraftAuthScope | null = null;
const logoutPending = ref(false);
const { error: logoutError, logout: performLogout } = useLogout({
  request: () => {
    logoutScope = retirement.capture();
    return logoutRequest();
  },
  redirect: (path) => {
    if (logoutScope && retirement.retire(logoutScope)) redirectTo(path);
  },
});
async function logout(): Promise<void> {
  if (logoutPending.value) return;
  logoutPending.value = true;
  try {
    await performLogout();
  } finally {
    logoutPending.value = false;
  }
}
usePushSessionRebind(() => props.workspaceId);

function onSwitch(event: Event): void {
  const select = event.target as HTMLSelectElement;
  const next = items.value.find((item) => item.id === select.value);
  if (next) followAppHref(landingPath(next.slug, props.active), router);
  // Keep the current workspace selected until the next page actually loads.
  select.value = props.workspaceId;
}
</script>

<template>
  <UDashboardGroup class="relative min-h-screen" :persistent="false">
    <UDashboardPanel id="workspace" :ui="{ body: 'p-0' }">
      <template #header>
        <div
          v-if="logoutError"
          role="alert"
          class="border-b border-default bg-muted px-4 py-2 text-sm text-muted"
        >
          {{ logoutError }}
        </div>
        <UDashboardNavbar
          as="header"
          :toggle="false"
          :ui="{
            root: 'h-auto flex-wrap py-3',
            left: 'min-w-0 max-w-full flex-wrap',
            right: 'min-w-0 max-w-full flex-wrap gap-3',
          }"
        >
          <template #left>
            <div class="flex min-w-0 max-w-full flex-wrap items-center gap-4">
              <AppLink to="/" class="underline underline-offset-2">{{ t("nav.backHome") }}</AppLink>
              <UNavigationMenu
                :items="navigation"
                :aria-label="t('nav.workspace')"
                :ui="{ list: 'flex-wrap' }"
              />
            </div>
          </template>
          <template #right>
            <div class="flex min-w-0 max-w-full flex-wrap items-center gap-3">
              <div v-if="items.length > 1" class="min-w-0 max-w-full">
                <label for="workspace-switch" class="sr-only">{{ t("workspace.switch") }}</label>
                <select
                  id="workspace-switch"
                  class="max-w-full rounded-md border border-default bg-default px-2 py-1 text-sm"
                  :value="workspaceId"
                  @change="onSwitch"
                >
                  <option v-for="item in items" :key="item.id" :value="item.id">{{
                    item.name
                  }}</option>
                </select>
              </div>
              <span
                v-else
                class="min-w-0 max-w-full break-words font-medium"
                data-slot="workspace-name"
                >{{ workspaceName }}</span
              >
              <PersonalInputDialog :workspace-id="workspaceId" />
              <SearchPalette :slug="slug" :workspace-id="workspaceId" />
              <NotificationBell :slug="slug" :workspace-id="workspaceId" />
              <AppLink to="/settings/account" class="underline underline-offset-2">
                {{ t("settings.account") }}
              </AppLink>
              <UButton
                size="sm"
                variant="outline"
                color="neutral"
                :disabled="logoutPending"
                @click="logout"
                >{{ t("nav.logout") }}</UButton
              >
            </div>
          </template>
        </UDashboardNavbar>
      </template>
      <template #body>
        <main class="flex-1 p-4">
          <slot />
        </main>
      </template>
      <template #footer>
        <footer class="border-t border-default px-4 py-3">
          <LegalNav />
        </footer>
      </template>
    </UDashboardPanel>
  </UDashboardGroup>
</template>
