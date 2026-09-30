<script setup lang="ts">
import { useNavigationError } from "../features/workspace/useNavigationError";
import { t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useInfiniteQuery, useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref } from "vue";
import { useRoute, useRouter } from "vue-router";
import { findProjectByKey, projectsQuery, workflowQuery } from "@/features/projects/queries";
import { invalidateTaskCaches } from "@/features/tasks/task-cache";
import {
  patchDateBody,
  patchTitleBody,
  patchTypeBody,
  type PatchTaskBody,
} from "@/features/tasks/task-edit-payload";
import { taskFieldValidationMessage, taskMutationErrorMessage } from "@/features/tasks/task-errors";
import { settleTaskPatch } from "@/features/tasks/task-patch-cache";
import { lookupQuery, resolveLookupTarget } from "@/features/tasks/lookup";
import {
  projectLabelsQuery,
  projectMilestonesQuery,
  taskListQuery,
  taskQuery,
} from "@/features/tasks/queries";
import { mergeTaskListPages } from "@/features/tasks/task-list-page";
import { membersQuery } from "@/lib/queries";
import { api, ensureOk, loadErrorMessage, ProblemError } from "@/lib/api";
import { itemPath, parseRef, projectsPath, projectTasksPath } from "@/lib/href";
import { collabRoomName } from "../collab/useCollabRoom";
import AppLink from "../components/AppLink.vue";
import QueryError from "../components/QueryError.vue";
import QueryLoading from "../components/QueryLoading.vue";
import WorkspaceShell from "../components/WorkspaceShell.vue";
import { useTaskStream } from "../composables/useTaskStream";
import ProjectDocumentView from "../features/documents/ProjectDocumentView.vue";
import TaskDetailView from "../features/tasks/TaskDetailView.vue";
import { leaveTo } from "../session/navigation";
import { useWorkspaceSession } from "../session/useWorkspaceSession";
import "@/features/projects/projects.css";

// `/w/:slug/GNT-1`: lookup a task or a project document (React TaskDetailPage).
// Task and document links render this Vue page; task lifecycle actions return
// to the connected task list through leaveTo.
const route = useRoute();
const router = useRouter();
const navigation = useNavigationError(() => route.fullPath);
const queryClient = useQueryClient();
const slug = computed(() => String(route.params.slug ?? ""));
const refParam = computed(() => String(route.params.ref ?? ""));

const session = useWorkspaceSession(slug);
const workspace = session.workspace;

const parsed = computed(() => parseRef(refParam.value));
const item = computed(() =>
  parsed.value?.kind === "item" && parsed.value.prefix !== "WIKI" ? parsed.value : null,
);
const displayId = computed(() => item.value?.displayId ?? "");

const fieldError = ref<string | null>(null);
const actionError = ref<string | null>(null);
const formEpoch = ref(0);

const workspaceId = computed(() => workspace.value?.id ?? "");
const projects = useQuery(() => projectsQuery(workspaceId.value));
const project = computed(() =>
  findProjectByKey(projects.data.value?.items, item.value?.prefix ?? ""),
);
const lookup = useQuery(() => lookupQuery(workspaceId.value, displayId.value));
const lookupTarget = computed(() =>
  lookup.isSuccess.value
    ? resolveLookupTarget(lookup.data.value?.items ?? [], displayId.value)
    : null,
);
const lookupTask = computed(() =>
  lookupTarget.value?.kind === "task" ? lookupTarget.value.item : null,
);
const projectDocument = computed(() =>
  lookupTarget.value?.kind === "project-document" ? lookupTarget.value.item : null,
);
const task = useQuery(() => taskQuery(workspaceId.value, lookupTask.value?.id ?? ""));
useTaskStream(workspaceId, () => project.value?.id ?? lookupTask.value?.projectId ?? undefined);
const workflow = useQuery(() =>
  workflowQuery(workspaceId.value, project.value?.id ?? lookupTask.value?.projectId ?? ""),
);
const taskPages = useInfiniteQuery(() =>
  taskListQuery(workspaceId.value, project.value?.id ?? task.data.value?.projectId ?? ""),
);
const parentItems = computed(
  () => mergeTaskListPages(taskPages.data.value?.pages ?? [])?.items ?? [],
);
const members = useQuery(() => ({
  ...membersQuery(workspaceId.value),
  enabled: Boolean(workspaceId.value),
}));
const labels = useQuery(() =>
  projectLabelsQuery(workspaceId.value, project.value?.id ?? task.data.value?.projectId ?? ""),
);
const milestones = useQuery(() =>
  projectMilestonesQuery(workspaceId.value, project.value?.id ?? task.data.value?.projectId ?? ""),
);

const taskId = computed(() => task.data.value?.id ?? "");
const projectId = computed(() => project.value?.id ?? task.data.value?.projectId ?? "");

async function afterMutation(): Promise<void> {
  await invalidateTaskCaches(queryClient, workspaceId.value, projectId.value, taskId.value);
}

function goTo(path: string): void {
  leaveTo(path, {
    assign: (url) => {
      window.location.assign(url);
    },
    push: (pathTo) => {
      navigation.run(() => router.push(pathTo));
    },
  });
}

const patchTask = useMutation({
  mutationFn: async (body: PatchTaskBody) =>
    ensureOk(
      await api.PATCH("/api/v1/workspaces/{workspace_id}/tasks/{task_id}", {
        params: { path: { workspace_id: workspaceId.value, task_id: taskId.value } },
        body,
      }),
    ),
  onSuccess: async (meta) => {
    fieldError.value = null;
    actionError.value = null;
    await settleTaskPatch(queryClient, workspaceId.value, projectId.value, meta);
  },
  onError: (err) => {
    actionError.value = taskMutationErrorMessage(err);
  },
});

const moveTask = useMutation({
  mutationFn: async (input: { statusId: string; expectedStatusId: string }) =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move", {
        params: { path: { workspace_id: workspaceId.value, task_id: taskId.value } },
        body: {
          statusId: input.statusId,
          expectedStatusId: input.expectedStatusId,
        },
      }),
    ),
  onSuccess: async () => {
    fieldError.value = null;
    actionError.value = null;
    await afterMutation();
  },
  onError: (err) => {
    actionError.value = taskMutationErrorMessage(err, "board.move.failed");
  },
});

const trashTask = useMutation({
  mutationFn: async () =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/trash", {
        params: { path: { workspace_id: workspaceId.value, task_id: taskId.value } },
      }),
    ),
  onSuccess: async () => {
    actionError.value = null;
    await invalidateTaskCaches(queryClient, workspaceId.value, projectId.value, taskId.value);
    const projectKey = project.value?.key ?? item.value?.prefix ?? "";
    if (projectKey) goTo(projectTasksPath(slug.value, projectKey));
  },
  onError: (err) => {
    actionError.value = taskMutationErrorMessage(err, "task.trash.failed");
  },
});

const cloneTask = useMutation({
  mutationFn: async () =>
    ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/clone", {
        params: { path: { workspace_id: workspaceId.value, task_id: taskId.value } },
      }),
    ),
  onSuccess: async (created) => {
    actionError.value = null;
    await invalidateTaskCaches(queryClient, workspaceId.value, projectId.value, taskId.value);
    goTo(itemPath(slug.value, created.displayId));
  },
  onError: (err) => {
    actionError.value = taskMutationErrorMessage(err, "task.clone.failed");
  },
});

const deleteTask = useMutation({
  mutationFn: async () =>
    ensureOk(
      await api.DELETE("/api/v1/workspaces/{workspace_id}/tasks/{task_id}", {
        params: { path: { workspace_id: workspaceId.value, task_id: taskId.value } },
      }),
    ),
  onSuccess: async () => {
    actionError.value = null;
    await invalidateTaskCaches(queryClient, workspaceId.value, projectId.value, taskId.value);
    const projectKey = project.value?.key ?? item.value?.prefix ?? "";
    if (projectKey) goTo(projectTasksPath(slug.value, projectKey));
  },
  onError: (err) => {
    actionError.value = taskMutationErrorMessage(err, "task.delete.failed");
  },
});

const pending = computed(
  () =>
    patchTask.isPending.value ||
    moveTask.isPending.value ||
    trashTask.isPending.value ||
    cloneTask.isPending.value ||
    deleteTask.isPending.value,
);

async function refetchAfterConflict(err: unknown): Promise<void> {
  if (err instanceof ProblemError && err.status === 409) {
    await afterMutation();
    formEpoch.value += 1;
  }
}

async function runPatch(body: PatchTaskBody): Promise<void> {
  if (!task.data.value?.canEdit) return;
  fieldError.value = null;
  actionError.value = null;
  try {
    await patchTask.mutateAsync(body);
  } catch (err) {
    await refetchAfterConflict(err);
  }
}

const projectsDenied = computed(
  () =>
    projects.isError.value &&
    projects.error.value instanceof ProblemError &&
    projects.error.value.status === 404,
);
const missingItem = computed(() => item.value == null);
const lookup404 = computed(
  () =>
    lookup.isError.value &&
    lookup.error.value instanceof ProblemError &&
    lookup.error.value.status === 404,
);
const lookupMiss = computed(() => lookup.isSuccess.value && lookupTarget.value?.kind === "miss");
const task404 = computed(
  () =>
    Boolean(lookupTask.value) &&
    task.isError.value &&
    task.error.value instanceof ProblemError &&
    task.error.value.status === 404,
);
const realNotFound = computed(
  () =>
    missingItem.value ||
    projectsDenied.value ||
    lookup404.value ||
    lookupMiss.value ||
    task404.value,
);

const taskReadOnly = computed(
  () =>
    !task.data.value?.canEdit ||
    task.data.value.archivedAt !== null ||
    project.value?.status === "archived",
);

async function onTitleBlur(title: string): Promise<void> {
  const parsedTitle = patchTitleBody(title);
  if (!parsedTitle.ok) {
    fieldError.value = taskFieldValidationMessage(parsedTitle.issue);
    return;
  }
  await runPatch(parsedTitle.body);
}

async function onStatusChange(statusId: string): Promise<void> {
  const current = task.data.value;
  if (!current || statusId === current.statusId) return;
  actionError.value = null;
  try {
    await moveTask.mutateAsync({
      statusId,
      expectedStatusId: current.statusId,
    });
  } catch (err) {
    await refetchAfterConflict(err);
  }
}

async function onPriorityChange(priority: string): Promise<void> {
  const current = task.data.value;
  if (!current || priority === current.priority) return;
  await runPatch({ priority });
}

async function onHierarchySave(type: string, parentId: string | null): Promise<void> {
  const parsedType = patchTypeBody(type, parentId);
  if (!parsedType.ok) {
    fieldError.value = taskFieldValidationMessage(parsedType.issue);
    return;
  }
  await runPatch(parsedType.body);
}

async function onDueDateBlur(value: string): Promise<void> {
  const current = task.data.value;
  if (!current) return;
  const parsedDate = patchDateBody(current, "dueDate", value);
  if (!parsedDate.ok) {
    fieldError.value = taskFieldValidationMessage(parsedDate.issue);
    return;
  }
  await runPatch(parsedDate.body);
}

async function onAddDependency(input: {
  blockedId: string;
  type: "FS" | "SS" | "FF";
  lagDays: number;
}): Promise<void> {
  const current = task.data.value;
  if (!current) return;
  fieldError.value = null;
  actionError.value = null;
  try {
    await ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/dependencies", {
        params: { path: { workspace_id: workspaceId.value, task_id: current.id } },
        body: input,
      }),
    );
    await afterMutation();
  } catch (err) {
    actionError.value = taskMutationErrorMessage(err, "task.dep.add.failed");
    await refetchAfterConflict(err);
    throw err;
  }
}

async function onRemoveDependency(edge: { blockerId: string; blockedId: string }): Promise<void> {
  fieldError.value = null;
  actionError.value = null;
  try {
    await ensureOk(
      await api.DELETE(
        "/api/v1/workspaces/{workspace_id}/tasks/{task_id}/dependencies/{blocked_id}",
        {
          params: {
            path: {
              workspace_id: workspaceId.value,
              task_id: edge.blockerId,
              blocked_id: edge.blockedId,
            },
          },
        },
      ),
    );
    await afterMutation();
  } catch (err) {
    actionError.value = taskMutationErrorMessage(err, "task.dep.remove.failed");
  }
}

async function onTrash(): Promise<void> {
  if (!window.confirm(`${t("task.trash.confirm.title")}\n${t("task.trash.confirm.body")}`)) return;
  try {
    await trashTask.mutateAsync();
  } catch {
    /* trashTask.onError already mapped the failure. */
  }
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
  >
    <QueryLoading v-if="projects.isLoading.value" />
    <QueryError
      v-if="projects.isError.value && !projectsDenied"
      :message="loadErrorMessage(projects.error.value)"
      @retry="projects.refetch()"
    />
    <p v-if="realNotFound" role="alert" class="task-form__alert">{{ t("task.error.notFound") }}</p>
    <ProjectDocumentView
      v-if="projectDocument && project"
      :key="collabRoomName(workspace.id, 'document', projectDocument.id)"
      :workspace-id="workspace.id"
      :slug="slug"
      :document-id="projectDocument.id"
      :project="{
        id: project.id,
        key: project.key,
        rootDocumentId: project.rootDocumentId ?? null,
        canEdit: project.canEdit,
        archived: project.status === 'archived',
      }"
    />
    <QueryLoading v-if="lookup.isLoading.value" />
    <QueryError
      v-if="lookup.isError.value && !lookup404"
      :message="loadErrorMessage(lookup.error.value)"
      @retry="lookup.refetch()"
    />
    <QueryLoading v-if="lookupTask && task.isLoading.value" />
    <QueryError
      v-if="lookupTask && task.isError.value && !task404"
      :message="loadErrorMessage(task.error.value)"
      @retry="task.refetch()"
    />
    <QueryError
      v-if="lookupTask && workflow.isError.value"
      :message="loadErrorMessage(workflow.error.value)"
      @retry="workflow.refetch()"
    />
    <TaskDetailView
      v-if="item && task.data.value"
      :key="collabRoomName(workspace.id, 'task', task.data.value.id)"
      :slug="slug"
      :workspace-id="workspace.id"
      :project-id="projectId"
      :project-key="project?.key ?? item.prefix"
      :project-name="project?.name"
      :current-user-id="session.me.value?.userId ?? ''"
      :task="task.data.value"
      :statuses="workflow.data.value?.statuses ?? []"
      :members="members.data.value?.items ?? []"
      :labels="labels.data.value?.items ?? []"
      :milestones="milestones.data.value?.items ?? []"
      :dependency-candidates="
        parentItems.map((row) => ({ id: row.id, number: row.number, title: row.title }))
      "
      :read-only="taskReadOnly"
      :can-edit="task.data.value.canEdit"
      :pending="pending"
      :field-error="fieldError"
      :action-error="actionError ?? navigation.error.value"
      :archive-pending="patchTask.isPending.value"
      :trash-pending="trashTask.isPending.value"
      :form-epoch="formEpoch"
      :clone-pending="cloneTask.isPending.value"
      :delete-pending="deleteTask.isPending.value"
      :on-title-blur="onTitleBlur"
      :on-status-change="onStatusChange"
      :on-priority-change="onPriorityChange"
      :on-hierarchy-save="onHierarchySave"
      :on-due-date-blur="onDueDateBlur"
      :on-assignees-change="(assigneeIds) => runPatch({ assigneeIds })"
      :on-labels-change="(labelIds) => runPatch({ labelIds })"
      :on-milestone-change="(milestoneId) => runPatch({ milestoneId })"
      :on-add-dependency="onAddDependency"
      :on-remove-dependency="onRemoveDependency"
      :on-archive-toggle="(archived) => runPatch({ archived })"
      :on-trash="onTrash"
      :on-clone="
        async () => {
          try {
            await cloneTask.mutateAsync();
          } catch {
            /* cloneTask.onError already mapped the failure. */
          }
        }
      "
      :on-delete="
        async () => {
          try {
            await deleteTask.mutateAsync();
          } catch {
            /* deleteTask.onError already mapped the failure. */
          }
        }
      "
    />
    <p v-if="realNotFound || projectDocument" class="task-home__note">
      <AppLink :to="project ? projectTasksPath(slug, project.key) : projectsPath(slug)">{{
        t("nav.projects")
      }}</AppLink>
    </p>
  </WorkspaceShell>
</template>
