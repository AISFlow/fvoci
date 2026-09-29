<script setup lang="ts">
import { t } from "@fvoci/i18n";
import {
  myTasksPath,
  projectsPath,
  searchPath,
  settingsPath,
  wikiPath,
  workspaceHomePath,
} from "@/lib/href";
import { usePushSessionRebind } from "../composables/usePushSessionRebind";
import LegalNav from "./LegalNav.vue";

// The workspace header of the React app's WorkspaceShell, reduced to links:
// every target is a React page, so each is a plain anchor (a full page load).
const props = withDefaults(
  defineProps<{
    slug: string;
    workspaceName: string;
    /** Needed to re-bind this browser's Web Push to the live session. */
    workspaceId?: string;
    /** The section the page belongs to. */
    active?: "wiki" | "projects" | "settings";
  }>(),
  { active: "projects" },
);

usePushSessionRebind(() => props.workspaceId);
</script>

<template>
  <div class="flex min-h-screen flex-col">
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
          <a
            :href="settingsPath(slug)"
            :class="active === 'settings' ? 'font-medium text-highlighted' : undefined"
            :aria-current="active === 'settings' ? 'page' : undefined"
            >{{ t("nav.settings") }}</a
          >
        </nav>
      </div>
      <div class="flex items-center gap-3">
        <span class="font-medium" data-slot="workspace-name">{{ workspaceName }}</span>
        <a href="/settings/account" class="underline underline-offset-2">
          {{ t("settings.account") }}
        </a>
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
