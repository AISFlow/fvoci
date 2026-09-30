<script setup lang="ts">
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useInfiniteQuery, useQuery } from "@tanstack/vue-query";
import { computed } from "vue";
import { useRoute } from "vue-router";
import { projectsQuery } from "@/features/projects/queries";
import {
  groupTasksByProject,
  myTasksQuery,
  workspaceStatusesQuery,
  workspaceLabelsQuery,
} from "@/features/tasks/my-tasks";
import { mergeTaskListPages } from "@/features/tasks/task-list-page";
import { loadErrorMessage, ProblemError } from "@/lib/api";
import { FALLBACK_TZ } from "@/lib/datetime";
import { meQuery, membersQuery } from "@/lib/queries";
import QueryError from "../components/QueryError.vue";
import QueryLoading from "../components/QueryLoading.vue";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import MyTaskRow from "../features/tasks/MyTaskRow.vue";
import { useWorkspaceSession } from "../session/useWorkspaceSession";
import "@/features/projects/projects.css";

const route = useRoute();
const slug = computed(() => String(route.params.slug ?? ""));
const session = useWorkspaceSession(slug);
const workspace = session.workspace;
const workspaceId = computed(() => workspace.value?.id ?? "");

const tasks = useInfiniteQuery(() => ({
  ...myTasksQuery(workspaceId.value),
  enabled: Boolean(workspaceId.value),
}));
const statuses = useQuery(() => ({
  ...workspaceStatusesQuery(workspaceId.value),
  enabled: Boolean(workspaceId.value),
}));
const projects = useQuery(() => ({
  ...projectsQuery(workspaceId.value),
  enabled: Boolean(workspaceId.value),
}));
const me = useQuery(meQuery);
const labels = useQuery(() => workspaceLabelsQuery(workspaceId.value));
const members = useQuery(() => ({
  ...membersQuery(workspaceId.value),
  enabled: Boolean(workspaceId.value),
}));

const items = computed(() => mergeTaskListPages(tasks.data.value?.pages ?? [])?.items ?? []);
const grouped = computed(() => groupTasksByProject(items.value));
const projectById = computed(
  () => new Map((projects.data.value?.items ?? []).map((project) => [project.id, project])),
);
const statusById = computed(
  () => new Map((statuses.data.value?.items ?? []).map((status) => [status.id, status])),
);
const timeZone = computed(() => me.data.value?.timezone ?? FALLBACK_TZ);
const cursorStale = computed(
  () =>
    tasks.error.value instanceof ProblemError &&
    tasks.error.value.status === 400 &&
    items.value.length > 0,
);

async function onLoadMore(): Promise<void> {
  if (cursorStale.value) await tasks.refetch();
  else await tasks.fetchNextPage();
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
    :slug="slug"
    :workspace-id="workspace.id"
    :workspace-name="workspace.name"
    active="myTasks"
  >
    <div class="task-home" data-testid="my-tasks">
      <div class="task-home__head">
        <h1 class="task-home__title">{{ t("task.mine") }}</h1>
      </div>
      <QueryLoading v-if="tasks.isLoading.value" />
      <QueryError
        v-else-if="tasks.isError.value && items.length === 0"
        :message="loadErrorMessage(tasks.error.value)"
        @retry="tasks.refetch()"
      />
      <p
        v-else-if="tasks.isSuccess.value && items.length === 0"
        class="break-keep text-lg font-semibold"
      >
        {{ t("task.assigned.empty") }}
      </p>
      <div class="flex flex-col gap-8">
        <section
          v-for="[projectId, projectItems] in grouped"
          :key="projectId"
          class="flex flex-col gap-1"
        >
          <h2 class="border-b border-default pb-2 text-sm font-medium">
            <span class="tabular-nums text-muted">{{
              projectById.get(projectId)?.key ?? "—"
            }}</span>
            <span v-if="projectById.get(projectId)?.name" class="ml-2 break-keep">{{
              projectById.get(projectId)?.name
            }}</span>
          </h2>
          <ul class="task-status-list">
            <li v-for="item in projectItems" :key="item.id">
              <MyTaskRow
                :slug="slug"
                :item-id="item.id"
                :title="item.title"
                :number="item.number"
                :project-key="projectById.get(projectId)?.key"
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
        </section>
      </div>
      <p v-if="cursorStale" role="status" class="break-keep text-sm text-muted">{{
        t("task.mine.cursorRestarted")
      }}</p>
      <div v-if="tasks.hasNextPage.value || cursorStale" class="flex flex-col items-start gap-2">
        <p v-if="tasks.isFetchNextPageError.value && !cursorStale" role="alert">{{
          t("task.mine.loadMoreFailed")
        }}</p>
        <UButton
          type="button"
          variant="outline"
          color="neutral"
          :disabled="tasks.isFetchingNextPage.value"
          @click="onLoadMore"
        >
          {{ tasks.isFetchingNextPage.value ? t("load.loading") : t("task.list.loadMore") }}
        </UButton>
      </div>
    </div>
  </WorkspaceShell>
</template>
