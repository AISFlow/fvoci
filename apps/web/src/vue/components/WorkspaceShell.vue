<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { landingPath } from "@/features/workspace/workspace-nav";
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
    active?: "wiki" | "projects";
  }>(),
  { active: "projects" },
);

const workspaces = useQuery(workspacesQuery);
const items = computed(() => workspaces.data.value?.items ?? []);
const { error: logoutError, logout } = useLogout();
usePushSessionRebind(() => props.workspaceId);

function onSwitch(event: Event): void {
  const next = items.value.find((item) => item.id === (event.target as HTMLSelectElement).value);
  if (next) window.location.assign(landingPath(next.slug, props.active));
}
</script>

<template>
  <div class="flex min-h-screen flex-col">
    <div v-if="logoutError" role="alert" class="border-b border-default bg-muted px-4 py-2 text-sm text-muted">
      {{ logoutError }}
    </div>
    <header class="flex flex-wrap items-center justify-between gap-3 border-b border-default px-4 py-3">
      <div class="flex flex-wrap items-center gap-4">
        <a href="/" class="underline underline-offset-2">{{ t("nav.backHome") }}</a>
        <nav class="flex flex-wrap items-center gap-3" :aria-label="t('nav.workspace')">
          <a :href="workspaceHomePath(slug)">{{ t("nav.home") }}</a>
          <a
            :href="wikiPath(slug)"
            :class="active === 'wiki' ? 'font-medium text-highlighted' : undefined"
            :aria-current="active === 'wiki' ? 'page' : undefined"
            >{{ t("nav.wiki") }}</a
          >
          <a
            :href="projectsPath(slug)"
            :class="active === 'projects' ? 'font-medium text-highlighted' : undefined"
            :aria-current="active === 'projects' ? 'page' : undefined"
            >{{ t("nav.projects") }}</a
          >
          <a :href="myTasksPath(slug)">{{ t("task.mine") }}</a>
          <a :href="searchPath(slug)">{{ t("nav.search") }}</a>
          <a :href="settingsPath(slug)">{{ t("nav.settings") }}</a>
        </nav>
      </div>
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
        <a href="/settings/account" class="underline underline-offset-2">
          {{ t("settings.account") }}
        </a>
        <UButton size="sm" variant="outline" color="neutral" @click="logout">{{ t("nav.logout") }}</UButton>
      </div>
    </header>
    <main class="flex-1 p-4">
      <slot />
    </main>
    <footer class="border-t border-default px-4 py-3">
      <LegalNav />
    </footer>
  </div>
</template>
