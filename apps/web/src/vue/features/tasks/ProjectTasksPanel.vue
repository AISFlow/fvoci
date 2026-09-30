<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import type { ProjectListItem } from "@/features/projects/queries";
import { backlogStatusId, workflowQuery } from "@/features/projects/queries";
import { viewConfigOf } from "@/features/tasks/project-views";
import {
  projectLabelsQuery,
  projectMilestonesQuery,
  taskListQuery,
  type CreateTaskBody,
} from "@/features/tasks/queries";
import { mergeTaskListPages } from "@/features/tasks/task-list-page";
import type { TaskCreateBody } from "@/features/tasks/create-payload";
import type { components } from "@/generated/api";
import { api, ensureOk, loadErrorMessage, ProblemError, problemMessage } from "@/lib/api";
import { FALLBACK_TZ } from "@/lib/datetime";
import { formatDisplayId, itemPath } from "@/lib/href";
import { membersQuery, meQuery } from "@/lib/queries";
import { collectionFieldsQuery, projectCollectionQuery, type ProjectView } from "@/lib/queries/collections";
import { encodeViewQueryParam, parseViewQueryParam, type ViewQuery } from "@/lib/view-query";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import ProjectMilestonesSection from "../projects/ProjectMilestonesSection.vue";
import ProjectGroupsSection from "../projects/ProjectGroupsSection.vue";
import TaskCreateDialog from "./TaskCreateDialog.vue";
import TaskFilters from "./TaskFilters.vue";
import TaskList from "./TaskList.vue";
import TaskSavedViews from "./TaskSavedViews.vue";
import "@/features/projects/projects.css";

type Workspace = components["schemas"]["WorkspaceListItemResponse"];

const props = defineProps<{
  slug: string;
  workspace: Workspace;
  project: ProjectListItem;
}>();

const route = useRoute();
const router = useRouter();
const queryClient = useQueryClient();
const createStatusId = ref<string | null>(null);

const rawQuery = computed(() => {
  const value = route.query.query;
  return typeof value === "string" ? value : null;
});
const parsedQuery = computed(() => parseViewQueryParam(rawQuery.value));
const viewQuery = computed<ViewQuery>(() => parsedQuery.value ?? { filters: {}, sort: [] });
const encodedQuery = computed(() => encodeViewQueryParam(viewQuery.value));
const selectedViewId = computed(() => {
  const value = route.query.view;
  return typeof value === "string" ? value : null;
});

const workflow = useQuery(() => workflowQuery(props.workspace.id, props.project.id));
const tasks = useInfiniteQuery(() => taskListQuery(props.workspace.id, props.project.id, encodedQuery.value));
const me = useQuery(() => meQuery);
const members = useQuery(() => membersQuery(props.workspace.id));
const labels = useQuery(() => projectLabelsQuery(props.workspace.id, props.project.id));
const milestones = useQuery(() => projectMilestonesQuery(props.workspace.id, props.project.id));
const collection = useQuery(() => projectCollectionQuery(props.workspace.id, props.project.id));
const fields = useQuery(() => collectionFieldsQuery(props.workspace.id, collection.data.value?.id ?? ""));

const taskPages = computed(() => mergeTaskListPages(tasks.data.value?.pages ?? []));
const firstPageFailed = computed(() => tasks.isError.value && !tasks.isFetchNextPageError.value);

function applyQuery(next: ViewQuery, viewId: string | null | undefined = selectedViewId.value): void {
  const query = { ...route.query } as Record<string, string>;
  const encoded = encodeViewQueryParam(next);
  if (encoded) query.query = encoded;
  else delete query.query;
  if (viewId) query.view = viewId;
  else delete query.view;
  void router.replace({ query });
}

const createTask = useMutation({
  mutationFn: async (body: CreateTaskBody) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/projects/{project_id}/tasks", {
        params: {
          path: { workspace_id: props.workspace.id, project_id: props.project.id },
        },
        body,
      }),
    ),
  onSuccess: async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["tasks", props.workspace.id, props.project.id] }),
      queryClient.invalidateQueries({ queryKey: ["projects", props.workspace.id] }),
    ]);
  },
});

function onSelectView(view: ProjectView | null): void {
  if (view) applyQuery(viewConfigOf(view), view.id);
  else applyQuery(viewQuery.value, null);
}

async function onCreateSubmit(values: TaskCreateBody): Promise<void> {
  if (!createStatusId.value) return;
  try {
    const created = await createTask.mutateAsync({
      title: values.title,
      type: values.type,
      statusId: createStatusId.value,
    });
    createStatusId.value = null;
    await router.push(itemPath(props.slug, formatDisplayId(props.project.key, created.number)));
  } catch {
    // Keep the dialog open; mutation error is shown in the form.
  }
}

function onCreateClose(): void {
  createStatusId.value = null;
  createTask.reset();
}

function retryList(): void {
  if (workflow.isError.value) void workflow.refetch();
  if (firstPageFailed.value) void tasks.refetch();
}
</script>

<template>
  <TaskSavedViews
    :workspace-id="workspace.id"
    :project-id="project.id"
    :query="viewQuery"
    :selected-id="selectedViewId"
    @select="onSelectView"
  />
  <TaskFilters
    :query="viewQuery"
    :statuses="workflow.data.value?.statuses ?? []"
    :labels="labels.data.value?.items ?? []"
    :milestones="milestones.data.value?.items ?? []"
    :members="members.data.value?.items ?? []"
    :fields="fields.data.value?.items ?? []"
    :time-zone="me.data.value?.timezone ?? FALLBACK_TZ"
    @change="applyQuery($event)"
  />
  <p v-if="parsedQuery === null" role="alert" class="task-form__alert">{{ t("task.filter.lastValidResults") }}</p>
  <QueryLoading v-if="workflow.isLoading.value || tasks.isLoading.value" />
  <QueryError
    v-if="workflow.isError.value || firstPageFailed"
    :message="loadErrorMessage(workflow.error.value ?? tasks.error.value)"
    @retry="retryList"
  />
  <TaskList
    v-if="!workflow.isLoading.value && !tasks.isLoading.value && !workflow.isError.value && !firstPageFailed && taskPages"
    :slug="slug"
    :project-key="project.key"
    :items="taskPages.items"
    :status-counts="taskPages.statusCounts"
    :statuses="workflow.data.value?.statuses ?? []"
    :can-create="project.status === 'active'"
    :default-status-id="backlogStatusId(workflow.data.value?.statuses ?? [])"
    :has-more="tasks.hasNextPage.value"
    :load-more-pending="tasks.isFetchingNextPage.value"
    :load-more-error="tasks.isFetchNextPageError.value ? loadErrorMessage(tasks.error.value) : null"
    @create="
      createTask.reset();
      createStatusId = $event;
    "
    @load-more="tasks.fetchNextPage()"
  />
  <TaskCreateDialog
    :open="createStatusId !== null"
    :project-key="project.key"
    :pending="createTask.isPending.value"
    :error="
      createTask.isError.value
        ? createTask.error.value instanceof ProblemError
          ? problemMessage(createTask.error.value, 'task.create.failed')
          : t('error.network')
        : null
    "
    @close="onCreateClose"
    @submit="onCreateSubmit"
  />
  <ProjectMilestonesSection :workspace-id="workspace.id" :project-id="project.id" :can-manage="project.status === 'active' && project.canEdit" />
  <ProjectGroupsSection :workspace-id="workspace.id" :project-id="project.id" :can-manage="workspace.role === 'admin' || workspace.role === 'owner'" />
</template>
