<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UCard from "@nuxt/ui/components/Card.vue";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { useRoute } from "vue-router";
import { projectsQuery } from "@/features/projects/queries";
import {
  assignedTasksPreviewQuery,
  workspaceLabelsQuery,
  workspaceStatusesQuery,
} from "@/features/tasks/my-tasks";
import { FALLBACK_TZ, formatInstant } from "@/lib/datetime";
import { wikiDiscoveryQuery } from "@/lib/queries/documents";
import { membersQuery } from "@/lib/queries";
import { projectsPath, projectTasksPath } from "@/lib/href";
import { loadErrorMessage } from "@/lib/api";
import { recentQuery, starsQuery } from "@/lib/queries/share";
import AppLink from "../components/AppLink.vue";
import QueryError from "../components/QueryError.vue";
import QueryLoading from "../components/QueryLoading.vue";
import MyTaskRow from "../features/tasks/MyTaskRow.vue";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import WorkspaceEntrance from "../features/projects/WorkspaceEntrance.vue";
import { useWorkspaceSession } from "../session/useWorkspaceSession";
import "@/features/share/share.css";
import "@/features/projects/projects.css";

const route = useRoute();
const slug = computed(() => String(route.params.slug ?? ""));
const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const workspaceId = computed(() => workspace.value?.id ?? "");

const stars = useQuery(() => starsQuery(workspaceId.value));
const recent = useQuery(() => recentQuery(workspaceId.value, 8));
const projects = useQuery(() => projectsQuery(workspaceId.value));

const documents = useQuery(() => wikiDiscoveryQuery(workspaceId.value));
const documentCount = computed(() =>
  documents.isSuccess.value ? documents.data.value?.items.length : "—",
);
const assigned = useQuery(() => assignedTasksPreviewQuery(workspaceId.value));
const labels = useQuery(() => workspaceLabelsQuery(workspaceId.value));
const statuses = useQuery(() => workspaceStatusesQuery(workspaceId.value));
const members = useQuery(() => ({
  ...membersQuery(workspaceId.value),
  enabled: Boolean(workspaceId.value),
}));
const timeZone = computed(() => session.me.value?.timezone ?? FALLBACK_TZ);
const projectById = computed(
  () => new Map((projects.data.value?.items ?? []).map((project) => [project.id, project])),
);
const statusById = computed(
  () => new Map((statuses.data.value?.items ?? []).map((status) => [status.id, status])),
);
const projectGroups = computed(() => [
  {
    status: "active",
    label: t("nav.projects"),
    items: projects.data.value?.items.filter((project) => project.status === "active") ?? [],
  },
  {
    status: "archived",
    label: t("project.archived.badge"),
    items: projects.data.value?.items.filter((project) => project.status === "archived") ?? [],
  },
]);
const projectCount = computed(() =>
  projects.isSuccess.value ? projects.data.value?.items.length : "—",
);
const openTaskCount = computed(() =>
  projects.isSuccess.value
    ? projects.data.value?.items.reduce((sum, project) => sum + project.openTaskCount, 0)
    : "—",
);

const projectKeyById = computed(
  () =>
    new Map(
      (projects.data.value?.items ?? []).map((project) => [project.id, project.key] as const),
    ),
);

const starItems = computed(() =>
  (stars.data.value?.items ?? []).map((star) => ({
    key: `${star.type}:${star.targetId}`,
    type: star.type,
    projectId: star.projectId,
    number: star.number,
    title: star.title,
  })),
);

const recentItems = computed(() =>
  (recent.data.value?.items ?? []).map((item) => ({
    key: `${item.type}:${item.id}`,
    type: item.type,
    projectId: item.projectId,
    number: item.number,
    title: item.title,
    meta: formatInstant(item.updatedAt, timeZone.value, { month: "numeric", day: "numeric" }),
  })),
);
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
    :slug="slug"
    :workspace-id="workspace.id"
    :workspace-name="workspace.name"
    active="home"
  >
    <div class="entrance">
      <h1 class="project-home__title">{{ workspace.name }}</h1>
      <p class="flex flex-wrap gap-3 text-sm text-muted" data-testid="workspace-totals">
        <span>{{ t("entrance.projectCount", { count: projectCount ?? "—" }) }}</span>
        <span>{{ t("entrance.documentCount", { count: documentCount ?? "—" }) }}</span>
        <span>{{ t("entrance.openTaskCount", { count: openTaskCount ?? "—" }) }}</span>
      </p>
      <QueryError
        v-if="documents.isError.value"
        :message="loadErrorMessage(documents.error.value)"
        @retry="() => void documents.refetch()"
      />
      <div class="grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(18rem,1fr)]">
        <UCard>
          <template #header>
            <div class="flex items-center justify-between gap-3">
              <h2 class="font-semibold">{{ t("nav.projects") }}</h2>
              <UButton :to="projectsPath(slug)" variant="outline" color="neutral">{{
                t("project.new")
              }}</UButton>
            </div>
          </template>
          <QueryLoading v-if="projects.isLoading.value" />
          <QueryError
            v-else-if="projects.isError.value"
            :message="loadErrorMessage(projects.error.value)"
            @retry="() => void projects.refetch()"
          />
          <p v-else-if="projects.data.value?.items.length === 0" class="text-sm text-muted">{{
            t("entrance.projects.empty")
          }}</p>
          <template v-else>
            <section v-for="group in projectGroups" :key="group.status" class="mb-4 last:mb-0">
              <h3
                v-if="group.status === 'archived' && group.items.length"
                class="mb-2 text-sm text-muted"
                >{{ group.label }}</h3
              >
              <ul class="flex flex-col gap-2">
                <li v-for="project in group.items" :key="project.id">
                  <AppLink
                    :to="projectTasksPath(slug, project.key)"
                    class="flex items-center gap-2 rounded p-2 hover:bg-elevated"
                  >
                    <span class="font-mono text-xs text-muted">{{ project.key }}</span>
                    <span class="flex-1">{{ project.name }}</span>
                    <span v-if="project.visibility === 'private'" class="text-xs text-muted">{{
                      t("project.visibility.private")
                    }}</span>
                    <span class="text-xs text-muted">{{
                      t("entrance.openTaskCount", { count: project.openTaskCount })
                    }}</span>
                  </AppLink>
                </li>
              </ul>
            </section>
          </template>
        </UCard>
        <UCard>
          <template #header
            ><h2 class="font-semibold">{{ t("task.mine") }}</h2></template
          >
          <QueryLoading v-if="assigned.isLoading.value" />
          <QueryError
            v-else-if="assigned.isError.value"
            :message="loadErrorMessage(assigned.error.value)"
            @retry="() => void assigned.refetch()"
          />
          <p v-else-if="assigned.data.value?.items.length === 0" class="text-sm text-muted">{{
            t("task.assigned.empty")
          }}</p>
          <ul v-else data-testid="workspace-assigned">
            <li v-for="item in assigned.data.value?.items ?? []" :key="item.id">
              <MyTaskRow
                :slug="slug"
                :item-id="item.id"
                :title="item.title"
                :number="item.number"
                :project-key="projectById.get(item.projectId)?.key"
                :status-name="statusById.get(item.statusId)?.name"
                :due-date="item.dueDate"
                :due-at="item.dueAt"
                :time-zone="timeZone"
                :type="item.type"
                :priority="item.priority"
                :labels="
                  (labels.data.value?.items ?? []).filter((label) =>
                    item.labelIds.includes(label.id),
                  )
                "
                :assignee-ids="item.assigneeIds"
                :members="members.data.value?.items ?? []"
              />
            </li>
          </ul>
        </UCard>
      </div>
      <WorkspaceEntrance
        :slug="slug"
        :heading="t('star.home')"
        :empty="t('entrance.stars.emptyHint')"
        :items="starItems"
        :loading="stars.isLoading.value"
        :error="stars.isError.value ? loadErrorMessage(stars.error.value) : null"
        :on-retry="() => void stars.refetch()"
        :project-key-by-id="projectKeyById"
      />
      <WorkspaceEntrance
        :slug="slug"
        :heading="t('recent.title')"
        :empty="t('recent.empty')"
        :items="recentItems"
        :loading="recent.isLoading.value"
        :error="recent.isError.value ? loadErrorMessage(recent.error.value) : null"
        :on-retry="() => void recent.refetch()"
        :project-key-by-id="projectKeyById"
      />
    </div>
  </WorkspaceShell>
</template>
