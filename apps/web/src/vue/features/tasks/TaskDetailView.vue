<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed, ref, useTemplateRef, watch } from "vue";
import type { WorkflowStatus } from "@/features/projects/queries";
import { taskOriginsQuery } from "@/features/collections/origin-api";
import {
  persistTaskBodyBeforeArchive,
  runArchiveWithBodyPersist,
} from "@/features/tasks/task-archive-persist";
import type { LabelItem, MilestoneItem, TaskDetail, TaskListItem } from "@/features/tasks/queries";
import { collabUserOf } from "@/features/documents/collab-model";
import type { MemberOutput } from "@/lib/contracts";
import { ProblemError } from "@/lib/api";
import { projectTasksPath } from "@/lib/href";
import { meQuery, workspacesQuery } from "@/lib/queries";
import { useCollabRoom, collabRoomName } from "../../collab/useCollabRoom";
import AppLink from "../../components/AppLink.vue";
import ConfirmActionButton from "../../components/ConfirmActionButton.vue";
import PersonalTransferDialog from "../capture/PersonalTransferDialog.vue";
import {
  taskTransferDocument,
  taskTransferPrepare,
  type TaskHostSnapshot,
} from "../capture/personal-transfer-command";
import TaskCollectionProperties from "../collections/TaskCollectionProperties.vue";
import TaskActivityPanel from "../comments/TaskActivityPanel.vue";
import OriginPanel from "../documents/OriginPanel.vue";
import StarToggle from "../documents/StarToggle.vue";
import TaskAttachmentsPanel from "./TaskAttachmentsPanel.vue";
import TaskBacklinks from "./TaskBacklinks.vue";
import TaskBodyEditor from "./TaskBodyEditor.vue";
import TaskDetailForm from "./TaskDetailForm.vue";
import TaskTimeEntries from "./TaskTimeEntries.vue";
import TaskStopwatch from "./TaskStopwatch.vue";
import "@/features/projects/projects.css";

const props = defineProps<{
  slug: string;
  workspaceId: string;
  projectId: string;
  projectKey: string;
  projectName?: string;
  currentUserId: string;
  task: TaskDetail;
  statuses: readonly WorkflowStatus[];
  members: readonly MemberOutput[];
  labels: readonly LabelItem[];
  milestones: readonly MilestoneItem[];
  dependencyCandidates: readonly Pick<TaskListItem, "id" | "number" | "title">[];
  readOnly: boolean;
  canEdit: boolean;
  pending?: boolean;
  fieldError?: string | null;
  actionError?: string | null;
  archivePending?: boolean;
  trashPending?: boolean;
  formEpoch?: number;
  clonePending?: boolean;
  deletePending?: boolean;
  onTitleBlur: (title: string) => void | Promise<void>;
  onStatusChange: (statusId: string) => void | Promise<void>;
  onPriorityChange: (priority: string) => void | Promise<void>;
  onHierarchySave: (type: string, parentId: string | null) => void | Promise<void>;
  onDueDateBlur: (
    value: string,
    expectedDates: Pick<TaskDetail, "startDate" | "dueDate" | "dueAt">,
  ) => void | Promise<void>;
  onAssigneesChange: (assigneeIds: string[]) => void | Promise<void>;
  onLabelsChange: (labelIds: string[]) => void | Promise<void>;
  onMilestoneChange: (milestoneId: string | null) => void | Promise<void>;
  onAddDependency: (input: {
    blockedId: string;
    type: "FS" | "SS" | "FF";
    lagDays: number;
  }) => void | Promise<void>;
  onRemoveDependency: (edge: { blockerId: string; blockedId: string }) => void | Promise<void>;
  onArchiveToggle: (archived: boolean) => void | Promise<void>;
  onTrash: () => void | Promise<void>;
  onClone: () => void | Promise<void>;
  onDelete: () => Promise<void>;
}>();

// One Y.Doc and one provider for this task body; the parent keys this
// component by the room name so a move to another item tears it down.
const me = useQuery(meQuery);
const collabUser = computed(() => {
  const data = me.data.value;
  return data ? collabUserOf(data.userId, formatPersonName(data, data.locale)) : null;
});
const room = useCollabRoom(
  collabRoomName(props.workspaceId, "task", props.task.id),
  collabUser,
  () => {
    const actor = me.data.value;
    if (
      !actor ||
      (me.error.value instanceof ProblemError && [401, 403].includes(me.error.value.status))
    )
      return null;
    return {
      roomName: collabRoomName(props.workspaceId, "task", props.task.id),
      actorId: actor.userId,
      sessionId: actor.sessionId,
      writable: me.isError.value
        ? null
        : props.canEdit && !props.readOnly && props.task.archivedAt == null,
    };
  },
);
const session = room.session;

const archivePersisting = ref(false);
const archivePersistError = ref<string | null>(null);
let archiveInFlight = false;

const pageReadOnly = computed(() => props.readOnly);
// HTTP metadata rights are independent of the collaboration connection's grant.
const metadataReadOnly = computed(() => pageReadOnly.value || archivePersisting.value);
const bodyReadOnly = computed(
  () => pageReadOnly.value || (session.value?.readOnly ?? false) || archivePersisting.value,
);
const pageEditable = computed(() => !pageReadOnly.value && props.task.archivedAt == null);
const archiveBusy = computed(() => props.archivePending || archivePersisting.value);

// Transfer from the task page: only a personal task with its single origin
// document. The mounted form's getter supplies its metadata drafts; the host
// fences its own identity, auth session and pending flags around the body's
// existing durable save (the archive barrier). Nothing is saved or discarded.
const workspaces = useQuery(workspacesQuery);
const taskOrigins = useQuery(() =>
  taskOriginsQuery(props.workspaceId, { taskId: props.task.id }, null),
);
const transferDocumentId = computed(() =>
  taskTransferDocument(
    workspaces.data.value?.items,
    props.workspaceId,
    props.task.id,
    taskOrigins.data.value,
  ),
);
// The mounted form's exposed getter (W5 TaskDetailForm getMetadataDraftState).
const form = useTemplateRef<{
  getMetadataDraftState: () => NonNullable<TaskHostSnapshot["draft"]>;
}>("form");
const authRetired = computed(
  () => me.error.value instanceof ProblemError && me.error.value.status === 401,
);
let hostGeneration = 0;
watch(
  [
    () => props.workspaceId,
    () => props.task.id,
    () => props.formEpoch,
    () => me.data.value?.userId,
    () => me.data.value?.sessionId,
    authRetired,
  ],
  () => {
    hostGeneration += 1;
  },
  { flush: "sync" },
);
function readTaskHost(): TaskHostSnapshot {
  return {
    workspaceId: props.workspaceId,
    taskId: props.task.id,
    actorId: authRetired.value ? "" : (me.data.value?.userId ?? ""),
    sessionId: authRetired.value ? "" : (me.data.value?.sessionId ?? ""),
    generation: hostGeneration,
    busy:
      props.pending ||
      archiveBusy.value ||
      archiveInFlight ||
      props.trashPending ||
      props.clonePending ||
      props.deletePending,
    draft: form.value?.getMetadataDraftState() ?? null,
    bodyGeneration: session.value?.generation ?? null,
    bodyPending: session.value?.pending ?? false,
  };
}
async function persistTransferBody(): Promise<boolean> {
  try {
    await persistTaskBodyBeforeArchive({
      pageEditable: pageEditable.value,
      session: session.value,
      collabUser: collabUser.value,
    });
    return true;
  } catch {
    return false;
  }
}
function prepareTransfer(): Promise<boolean> {
  return taskTransferPrepare(readTaskHost, persistTransferBody);
}

async function handleArchiveToggle(archived: boolean): Promise<void> {
  if (archiveInFlight || props.archivePending || archivePersisting.value) return;
  if (!archived) {
    archivePersistError.value = null;
    archiveInFlight = true;
    archivePersisting.value = true;
    try {
      await props.onArchiveToggle(false);
    } finally {
      archivePersisting.value = false;
      archiveInFlight = false;
    }
    return;
  }
  archivePersistError.value = null;
  archiveInFlight = true;
  archivePersisting.value = true;
  try {
    await runArchiveWithBodyPersist({
      pageEditable: pageEditable.value,
      session: session.value,
      collabUser: collabUser.value,
      archive: async () => {
        await props.onArchiveToggle(true);
      },
    });
  } catch {
    archivePersistError.value = t("task.archive.persistFailed");
  } finally {
    archivePersisting.value = false;
    archiveInFlight = false;
  }
}
</script>

<template>
  <div class="task-home">
    <p v-if="archivePersistError" role="alert" class="task-form__alert">{{
      archivePersistError
    }}</p>
    <nav class="task-home__crumb" :aria-label="t('nav.breadcrumb')">
      <AppLink :to="projectTasksPath(slug, projectKey)">{{ projectName ?? projectKey }}</AppLink>
      <span aria-hidden="true"> / </span>
      <span>{{ task.title }}</span>
    </nav>
    <h1 class="task-detail__title">{{ task.title }}</h1>
    <div>
      <StarToggle :workspace-id="workspaceId" type="task" :target-id="task.id" />
      <PersonalTransferDialog
        v-if="!readOnly && transferDocumentId"
        :workspace-id="workspaceId"
        :document-id="transferDocumentId"
        :prepare="prepareTransfer"
      />
    </div>
    <TaskDetailForm
      ref="form"
      :key="`${task.id}:${formEpoch ?? 0}`"
      :slug="slug"
      :workspace-id="workspaceId"
      :current-user-id="currentUserId"
      :project-id="projectId"
      :project-key="projectKey"
      :task="task"
      :statuses="statuses"
      :members="members"
      :labels="labels"
      :milestones="milestones"
      :dependency-candidates="dependencyCandidates"
      :read-only="metadataReadOnly"
      :can-edit="canEdit"
      :pending="pending"
      :field-error="fieldError"
      :action-error="actionError"
      :archive-pending="archiveBusy"
      :trash-pending="trashPending"
      :on-add-dependency="onAddDependency"
      @title-blur="onTitleBlur"
      @status-change="onStatusChange"
      @priority-change="onPriorityChange"
      @hierarchy-save="onHierarchySave"
      @due-date-blur="onDueDateBlur"
      @assignees-change="onAssigneesChange"
      @labels-change="onLabelsChange"
      @milestone-change="onMilestoneChange"
      @remove-dependency="onRemoveDependency"
      @archive-toggle="handleArchiveToggle"
      @trash="onTrash"
    />
    <div v-if="canEdit" class="flex flex-wrap gap-2" data-testid="task-detail-actions">
      <UButton
        v-if="!metadataReadOnly"
        size="sm"
        variant="outline"
        color="neutral"
        data-testid="task-clone"
        :disabled="clonePending || pending"
        @click="onClone()"
      >
        {{ t("task.clone") }}
      </UButton>
      <ConfirmActionButton
        v-if="task.archivedAt == null"
        :title="t('task.delete.confirm.title')"
        :description="t('task.delete.confirm.body')"
        :action-label="t('task.delete')"
        :disabled="deletePending || pending"
        :action="onDelete"
      >
        {{ t("task.delete") }}
      </ConfirmActionButton>
    </div>
    <TaskCollectionProperties
      :workspace-id="workspaceId"
      :task-id="task.id"
      :read-only="metadataReadOnly"
    />
    <TaskBodyEditor
      :workspace-id="workspaceId"
      :slug="slug"
      :task-id="task.id"
      :read-only="bodyReadOnly"
      :session="session"
      :collab-user="collabUser"
    />
    <TaskAttachmentsPanel
      :workspace-id="workspaceId"
      :task-id="task.id"
      :read-only="bodyReadOnly || task.archivedAt != null"
    />
    <TaskTimeEntries
      :workspace-id="workspaceId"
      :task-id="task.id"
      :members="members"
      :read-only="metadataReadOnly"
    />
    <TaskStopwatch
      :workspace-id="workspaceId"
      :task-id="task.id"
      :estimate="task.estimate"
      :read-only="metadataReadOnly"
    />
    <TaskActivityPanel
      v-if="currentUserId"
      :key="task.id"
      :workspace-id="workspaceId"
      :task-id="task.id"
      :current-user-id="currentUserId"
      :read-only="metadataReadOnly"
    />
    <TaskBacklinks :slug="slug" :workspace-id="workspaceId" :task-id="task.id" />
    <OriginPanel :slug="slug" :workspace-id="workspaceId" :task-id="task.id" hide-when-empty />
  </div>
</template>
