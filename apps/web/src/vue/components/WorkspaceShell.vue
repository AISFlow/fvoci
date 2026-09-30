<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import UDashboardGroup from "@nuxt/ui/components/DashboardGroup.vue";
import UDashboardPanel from "@nuxt/ui/components/DashboardPanel.vue";
import UDashboardNavbar from "@nuxt/ui/components/DashboardNavbar.vue";
import UNavigationMenu from "@nuxt/ui/components/NavigationMenu.vue";
import { isVueAppPath } from "@/app-boundary";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { useRouter } from "vue-router";
import AppLink from "./AppLink.vue";
import { followAppHref } from "../session/navigation";
import { landingPath, type WorkspaceNav } from "@/features/workspace/workspace-nav";
import {
  myTasksPath,
  projectsPath,
  searchPath,
  settingsPath,
  wikiPath,
  workspaceHomePath,
} from "@/lib/href";
import { workspacesQuery } from "@/lib/queries";
import LegalNav from "../features/shell/LegalNav.vue";
import NotificationBell from "../features/shell/NotificationBell.vue";
import SearchPalette from "../features/shell/SearchPalette.vue";
import { useLogout } from "../features/shell/useLogout";
import { usePushSessionRebind } from "../features/shell/usePushSessionRebind";

// The React app's workspace shell (features/workspace/workspace-shell.tsx):
// section links, the workspace switch, the search palette, the notification
// bell, account and logout, and the legal footer. Every link and switch
// target outside this app is a React page, reached with a full page load.
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
const items = computed(() => workspaces.data.value?.items ?? []);
// Nuxt UI Dashboard template, fixed 57e8a76e: layouts/default.vue menu
// and pages/index.vue panel/header slots, connected to the existing FVOCI paths.
const navigation = computed(() => [
  { label: t("nav.home"), to: workspaceHomePath(props.slug), active: props.active === "home" },
  { label: t("nav.wiki"), to: wikiPath(props.slug), active: props.active === "wiki" },
  { label: t("nav.projects"), to: projectsPath(props.slug), active: props.active === "projects" },
  { label: t("task.mine"), to: myTasksPath(props.slug), active: props.active === "myTasks" },
  { label: t("nav.search"), to: searchPath(props.slug), active: props.active === "search" },
  { label: t("nav.settings"), to: settingsPath(props.slug), active: props.active === "settings" },
].map((item) => ({ ...item, exact: true, external: !isVueAppPath(item.to) })));
const { error: logoutError, logout } = useLogout();
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
    <div v-if="logoutError" role="alert" class="border-b border-default bg-muted px-4 py-2 text-sm text-muted">
      {{ logoutError }}
    </div>
    <UDashboardNavbar as="header" :toggle="false" :ui="{ root: 'h-auto flex-wrap py-3', left: 'flex-wrap', right: 'flex-wrap gap-3' }">
      <template #left>
      <div class="flex flex-wrap items-center gap-4">
        <AppLink to="/" class="underline underline-offset-2">{{ t("nav.backHome") }}</AppLink>
        <UNavigationMenu :items="navigation" :aria-label="t('nav.workspace')" :ui="{ list: 'flex-wrap' }" />
      </div>
      </template>
      <template #right>
      <div class="flex flex-wrap items-center gap-3">
        <div v-if="items.length > 1">
          <label for="workspace-switch" class="sr-only">{{ t("workspace.switch") }}</label>
          <select
            id="workspace-switch"
            class="rounded-md border border-default bg-default px-2 py-1 text-sm"
            :value="workspaceId"
            @change="onSwitch"
          >
            <option v-for="item in items" :key="item.id" :value="item.id">{{ item.name }}</option>
          </select>
        </div>
        <span v-else class="font-medium" data-slot="workspace-name">{{ workspaceName }}</span>
        <SearchPalette :slug="slug" :workspace-id="workspaceId" />
        <NotificationBell :slug="slug" :workspace-id="workspaceId" />
        <AppLink to="/settings/account" class="underline underline-offset-2">
          {{ t("settings.account") }}
        </AppLink>
        <UButton size="sm" variant="outline" color="neutral" @click="logout">{{ t("nav.logout") }}</UButton>
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
