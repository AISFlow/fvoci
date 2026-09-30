<script setup lang="ts">
import type { I18nKey } from "@fvoci/i18n";
import { t } from "@fvoci/i18n";
import { computed } from "vue";
import {
  projectCollectionPath,
  projectFieldsPath,
  projectGanttPath,
  projectTasksPath,
  projectWorkflowPath,
} from "@/lib/href";
import AppLink from "./AppLink.vue";

export type ProjectViewTab =
  "tasks" | "table" | "board" | "calendar" | "gantt" | "fields" | "workflow";

// The React ProjectViewNav tabs. A tab of a Vue page is an in-app
// navigation; the others (project settings) are the React app's pages,
// reached with a full page load (AppLink).
const props = defineProps<{
  slug: string;
  projectKey: string;
  active: ProjectViewTab;
}>();

const tabs = computed<{ id: ProjectViewTab; href: string; label: I18nKey }[]>(() => [
  { id: "tasks", href: projectTasksPath(props.slug, props.projectKey), label: "nav.tasks" },
  {
    id: "table",
    href: projectCollectionPath(props.slug, props.projectKey, "table"),
    label: "collection.table",
  },
  {
    id: "board",
    href: projectCollectionPath(props.slug, props.projectKey, "board"),
    label: "collection.board",
  },
  {
    id: "calendar",
    href: projectCollectionPath(props.slug, props.projectKey, "calendar"),
    label: "collection.calendar",
  },
  { id: "gantt", href: projectGanttPath(props.slug, props.projectKey), label: "view.gantt" },
  {
    id: "fields",
    href: projectFieldsPath(props.slug, props.projectKey),
    label: "collection.fieldSettings",
  },
  {
    id: "workflow",
    href: projectWorkflowPath(props.slug, props.projectKey),
    label: "project.settings.workflow",
  },
]);
</script>

<template>
  <nav
    class="flex flex-wrap gap-1 border-b border-default"
    :aria-label="t('project.settings.navigation')"
  >
    <AppLink
      v-for="tab in tabs"
      :key="tab.id"
      :to="tab.href"
      class="-mb-px border-b-2 px-3 py-2 text-sm"
      :class="
        tab.id === active
          ? 'border-primary font-medium text-highlighted'
          : 'border-transparent text-muted hover:text-default'
      "
      :aria-current="tab.id === active ? 'page' : undefined"
      >{{ t(tab.label) }}</AppLink
    >
  </nav>
</template>
