<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, onBeforeUnmount, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { FALLBACK_TZ } from "@/lib/datetime";
import { itemPath, projectPath, projectsPath } from "@/lib/href";
import { parseViewQueryParam, withTitleFilter, EMPTY_VIEW_QUERY } from "@/lib/view-query";
import { shiftMonth } from "@/lib/year-month";
import { dateInZone } from "@/lib/zoned-date";
import ProjectViewTabs from "../components/ProjectViewTabs.vue";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import { useTaskStream } from "../composables/useTaskStream";
import GanttView from "../features/gantt/GanttView.vue";
import { useProjectRef } from "../session/useProjectRef";
import { useWorkspaceSession } from "../session/useWorkspaceSession";

const route = useRoute();
const router = useRouter();

const slug = computed(() => String(route.params.slug ?? ""));
const refParam = computed(() => String(route.params.ref ?? ""));

const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const projectRef = useProjectRef(() => workspace.value?.id, refParam);
const { project, notFound } = projectRef;
useTaskStream(
  () => workspace.value?.id,
  () => project.value?.id,
);

const timeZone = computed(() => session.me.value?.timezone || FALLBACK_TZ);
const weekStartsOn = computed<0 | 1>(() => (session.me.value?.weekStartsOn === 1 ? 1 : 0));

// "Today" in the user's time zone, refreshed each minute so a page left open
// past midnight moves its marker.
const now = ref(Date.now());
const clock = window.setInterval(() => {
  now.value = Date.now();
}, 60_000);
onBeforeUnmount(() => window.clearInterval(clock));
const today = computed(() => dateInZone(now.value, timeZone.value));

function queryInt(name: string, min: number, max: number): number | undefined {
  const raw = route.query[name];
  const value = typeof raw === "string" && /^\d{1,4}$/.test(raw) ? Number.parseInt(raw, 10) : Number.NaN;
  return Number.isInteger(value) && value >= min && value <= max ? value : undefined;
}

// The URL holds the month (y, m) and the view query; without them the page
// shows the current month in the user's time zone.
const year = computed(() => queryInt("y", 1, 9999) ?? Number(today.value.slice(0, 4)));
const month = computed(() => queryInt("m", 1, 12) ?? Number(today.value.slice(5, 7)));
const viewQuery = computed(() => {
  const raw = route.query.query;
  return parseViewQueryParam(typeof raw === "string" ? raw : null) ?? EMPTY_VIEW_QUERY;
});

function onShiftMonth(delta: -1 | 1): void {
  const next = shiftMonth(year.value, month.value, delta);
  void router.replace({ query: { ...route.query, y: String(next.year), m: String(next.month) } });
}

function onSearch(title: string): void {
  const next = withTitleFilter(viewQuery.value, title);
  const query = { ...route.query };
  if (next.filters.title) query.query = JSON.stringify(next);
  else delete query.query;
  void router.replace({ query });
}

function onOpenTask(displayId: string): void {
  window.location.assign(itemPath(slug.value, displayId));
}
</script>

<template>
  <p v-if="session.status.value === 'loading'" role="status" class="p-8 text-muted">{{ t("load.loading") }}</p>
  <div v-else-if="session.status.value === 'error'" class="p-8">
    <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
    <UButton size="sm" class="mt-2" @click="session.retry()">{{ t("load.retry") }}</UButton>
  </div>
  <WorkspaceShell v-else-if="workspace" :slug="slug" :workspace-name="workspace.name">
    <p v-if="notFound" role="alert" class="text-error">{{ t("project.notFound") }}</p>
    <div v-else-if="projectRef.failed.value">
      <p role="alert" class="text-muted">{{ t("load.failed") }}</p>
      <UButton size="sm" class="mt-2" @click="projectRef.retry()">{{ t("load.retry") }}</UButton>
    </div>
    <div v-else-if="project" class="flex flex-col gap-3">
      <p class="text-sm">
        <a :href="projectsPath(slug)" class="underline underline-offset-2">{{ t("nav.projects") }}</a>
        <span aria-hidden="true"> / </span>
        <a :href="projectPath(slug, project.key)" class="underline underline-offset-2" data-slot="project-link">{{
          project.key
        }}</a>
      </p>
      <ProjectViewTabs :slug="slug" :project-key="project.key" active="gantt" />
      <GanttView
        :workspace-id="workspace.id"
        :project-id="project.id"
        :project-key="project.key"
        :year="year"
        :month="month"
        :query="viewQuery"
        :week-starts-on="weekStartsOn"
        :time-zone="timeZone"
        :today="today"
        @shift-month="onShiftMonth"
        @search="onSearch"
        @open-task="onOpenTask"
      />
    </div>
    <p v-else role="status" class="text-muted">{{ t("load.loading") }}</p>
  </WorkspaceShell>
</template>
