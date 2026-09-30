<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref, watch } from "vue";
import type { WorkflowStatus } from "@/features/projects/queries";
import type { LabelItem, MilestoneItem, TaskDependency, TaskDetail, TaskListItem } from "@/features/tasks/queries";
import {
  PRIORITIES,
  clearsHierarchyParent,
  patchTypeBody,
  priorityLabel,
} from "@/features/tasks/task-edit-payload";
import { TASK_TYPES, TASK_TYPE_LABELS, isTaskType, type TaskType } from "@/features/tasks/task-types";
import type { MemberOutput } from "@/lib/contracts";
import { formatDisplayId, itemPath } from "@/lib/href";
import AppLink from "../../components/AppLink.vue";
import TaskParentSelect from "./TaskParentSelect.vue";
import "@/features/projects/projects.css";

const NONE = "";
const DEP_TYPES = ["FS", "SS", "FF"] as const;

const props = defineProps<{
  slug: string;
  workspaceId: string;
  projectId: string;
  projectKey: string;
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
  onAddDependency: (input: {
    blockedId: string;
    type: "FS" | "SS" | "FF";
    lagDays: number;
  }) => void | Promise<void>;
}>();

const emit = defineEmits<{
  titleBlur: [title: string];
  statusChange: [statusId: string];
  priorityChange: [priority: string];
  hierarchySave: [type: string, parentId: string | null];
  dueDateBlur: [value: string];
  assigneesChange: [assigneeIds: string[]];
  labelsChange: [labelIds: string[]];
  milestoneChange: [milestoneId: string | null];
  removeDependency: [edge: { blockerId: string; blockedId: string }];
  archiveToggle: [archived: boolean];
  trash: [];
}>();

const displayId = computed(() => formatDisplayId(props.projectKey, props.task.number));
const archived = computed(() => props.task.archivedAt !== null);
const draftType = ref<TaskType>(isTaskType(props.task.type) ? props.task.type : "task");
const draftParentId = ref<string | null>(props.task.parentId);
const draftAssigneeIds = ref<string[]>([...props.task.assigneeIds]);
const draftLabelIds = ref<string[]>([...props.task.labelIds]);
const hierarchyError = ref<string | null>(null);
const depFormOpen = ref(false);
const depBlockedId = ref(NONE);
const depType = ref<(typeof DEP_TYPES)[number]>("FS");
const depLagDays = ref(0);
const depLocalError = ref<string | null>(null);
const titleDraft = ref(props.task.title);
const dueDateDraft = ref(props.task.dueDate ?? "");

watch(
  () => [props.task.id, props.task.type, props.task.parentId] as const,
  () => {
    draftType.value = isTaskType(props.task.type) ? props.task.type : "task";
    draftParentId.value = props.task.parentId;
    hierarchyError.value = null;
  },
);

watch(
  () => props.task.assigneeIds,
  (ids) => {
    draftAssigneeIds.value = [...ids];
  },
);

watch(
  () => props.task.labelIds,
  (ids) => {
    draftLabelIds.value = [...ids];
  },
);

const showParent = computed(() => draftType.value !== "epic");
const hierarchyDirty = computed(() => draftType.value !== props.task.type || draftParentId.value !== props.task.parentId);
const dependencies = computed(() => (props.task.dependencies ?? []) as TaskDependency[]);
const parentCurrentTitle = computed(() => {
  const parent = props.task.parent;
  if (parent && draftParentId.value === parent.id) {
    return `${formatDisplayId(props.projectKey, parent.number)} ${parent.title}`;
  }
  return undefined;
});

function onTypeChange(event: Event): void {
  const next = (event.target as HTMLSelectElement).value;
  if (!isTaskType(next)) return;
  if (clearsHierarchyParent(draftType.value, next)) draftParentId.value = null;
  draftType.value = next;
  hierarchyError.value = null;
}

function cancelHierarchy(): void {
  draftType.value = isTaskType(props.task.type) ? props.task.type : "task";
  draftParentId.value = props.task.parentId;
  hierarchyError.value = null;
}

function saveHierarchy(): void {
  const parsed = patchTypeBody(draftType.value, draftParentId.value);
  if (!parsed.ok) {
    hierarchyError.value = t("task.parent.required");
    return;
  }
  emit("hierarchySave", draftType.value, parsed.body.parentId ?? null);
}

function onTitleBlur(): void {
  if (titleDraft.value !== props.task.title) emit("titleBlur", titleDraft.value);
}

function onTitleKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter") (event.target as HTMLInputElement).blur();
  if (event.key === "Escape") {
    titleDraft.value = props.task.title;
    (event.target as HTMLInputElement).blur();
  }
}

function onDueDateBlur(): void {
  if (dueDateDraft.value !== (props.task.dueDate ?? "")) emit("dueDateBlur", dueDateDraft.value);
}

function onDueDateKeydown(event: KeyboardEvent): void {
  if (event.key === "Enter") (event.target as HTMLInputElement).blur();
  if (event.key === "Escape") {
    dueDateDraft.value = props.task.dueDate ?? "";
    (event.target as HTMLInputElement).blur();
  }
}

function toggleAssignee(userId: string, checked: boolean): void {
  const next = checked
    ? draftAssigneeIds.value.filter((id) => id !== userId)
    : [...draftAssigneeIds.value, userId];
  draftAssigneeIds.value = next;
  emit("assigneesChange", next);
}

function toggleLabel(labelId: string, checked: boolean): void {
  const next = checked
    ? draftLabelIds.value.filter((id) => id !== labelId)
    : [...draftLabelIds.value, labelId];
  draftLabelIds.value = next;
  emit("labelsChange", next);
}

function onAddDependency(event: Event): void {
  event.preventDefault();
  if (depBlockedId.value === NONE) {
    depLocalError.value = t("dep.target.required");
    return;
  }
  depLocalError.value = null;
  void Promise.resolve(
    props.onAddDependency({
      blockedId: depBlockedId.value,
      type: depType.value,
      lagDays: Number.isFinite(depLagDays.value) ? Math.max(0, depLagDays.value) : 0,
    }),
  ).then(
    () => {
      depFormOpen.value = false;
      depBlockedId.value = NONE;
      depType.value = "FS";
      depLagDays.value = 0;
    },
    () => {
      /* parent actionError already maps the failure */
    },
  );
}

function onDepTypeChange(event: Event): void {
  const next = (event.target as HTMLSelectElement).value;
  if (next === "FS" || next === "SS" || next === "FF") depType.value = next;
}

function dependencyName(edge: TaskDependency): string {
  const otherId = edge.blockerId === props.task.id ? edge.blockedId : edge.blockerId;
  const other = props.dependencyCandidates.find((candidate) => candidate.id === otherId);
  return other ? `${formatDisplayId(props.projectKey, other.number)} ${other.title}` : otherId.slice(0, 8);
}
</script>

<template>
  <div class="task-detail">
    <div v-if="archived" class="task-detail__archived">
      <p class="task-home__note">{{ t("task.archive.detailStatus") }}</p>
      <UButton
        v-if="canEdit"
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="archivePending || pending"
        @click="emit('archiveToggle', false)"
      >
        {{ t("task.restore.action") }}
      </UButton>
    </div>
    <div class="task-form">
      <div class="task-form__field">
        <label for="task-edit-title" class="text-sm font-medium">{{ t("task.col.title") }}</label>
        <input
          id="task-edit-title"
          v-model="titleDraft"
          data-testid="task-edit-title"
          :aria-label="t('task.title')"
          :disabled="readOnly || pending"
          @blur="onTitleBlur"
          @keydown="onTitleKeydown"
        />
      </div>
      <div class="task-form__field">
        <label for="task-edit-status" class="text-sm font-medium">{{ t("task.col.status") }}</label>
        <select
          id="task-edit-status"
          data-testid="task-edit-status"
          :disabled="readOnly || pending"
          :value="task.statusId"
          @change="emit('statusChange', ($event.target as HTMLSelectElement).value)"
        >
          <option v-for="status in statuses" :key="status.id" :value="status.id">{{ status.name }}</option>
        </select>
      </div>
      <fieldset class="task-form__hierarchy" :disabled="readOnly || pending">
        <legend>{{ t("task.hierarchy.edit") }}</legend>
        <div class="task-form__field">
          <label for="task-edit-type" class="text-sm font-medium">{{ t("task.detail.type.label") }}</label>
          <select
            id="task-edit-type"
            data-testid="task-edit-type"
            :disabled="readOnly || pending"
            :value="draftType"
            @change="onTypeChange"
          >
            <option v-for="value in TASK_TYPES" :key="value" :value="value">{{ TASK_TYPE_LABELS[value] }}</option>
          </select>
        </div>
        <div v-if="showParent" class="task-form__field">
          <label for="task-edit-parent" class="text-sm font-medium">{{ t("task.parent.label") }}</label>
          <TaskParentSelect
            v-model="draftParentId"
            :workspace-id="workspaceId"
            :project-id="projectId"
            :child-type="draftType"
            :exclude-task-id="task.id"
            :current-title="parentCurrentTitle"
            :disabled="readOnly || pending"
          />
          <p v-if="task.parent" class="task-home__note">
            <AppLink :to="itemPath(slug, formatDisplayId(projectKey, task.parent.number))">
              {{ t("task.parent.current") }}: {{ formatDisplayId(projectKey, task.parent.number) }}
            </AppLink>
          </p>
        </div>
        <p v-if="hierarchyError" class="task-form__alert" role="alert" data-testid="task-edit-hierarchy-error">
          {{ hierarchyError }}
        </p>
        <div class="task-form__hierarchy-actions">
          <UButton
            size="sm"
            variant="outline"
            color="neutral"
            data-testid="task-edit-hierarchy-cancel"
            :disabled="readOnly || pending || !hierarchyDirty"
            @click="cancelHierarchy"
          >
            {{ t("task.hierarchy.cancel") }}
          </UButton>
          <UButton
            size="sm"
            data-testid="task-edit-hierarchy-save"
            :disabled="readOnly || pending || !hierarchyDirty"
            @click="saveHierarchy"
          >
            {{ t("task.hierarchy.save") }}
          </UButton>
        </div>
      </fieldset>
      <div class="task-form__field">
        <label for="task-edit-priority" class="text-sm font-medium">{{ t("task.priority") }}</label>
        <select
          id="task-edit-priority"
          data-testid="task-edit-priority"
          :disabled="readOnly || pending"
          :value="task.priority"
          @change="emit('priorityChange', ($event.target as HTMLSelectElement).value)"
        >
          <option v-for="value in PRIORITIES" :key="value" :value="value">{{ priorityLabel(value) }}</option>
        </select>
      </div>
      <div class="task-form__field">
        <label for="task-edit-due-date" class="text-sm font-medium">{{ t("task.dueAllDay") }}</label>
        <input
          id="task-edit-due-date"
          v-model="dueDateDraft"
          data-testid="task-edit-due-date"
          type="date"
          :disabled="readOnly || pending"
          @blur="onDueDateBlur"
          @keydown="onDueDateKeydown"
        />
      </div>
      <fieldset class="task-form__field" :disabled="readOnly" data-testid="task-edit-assignees">
        <legend>{{ t("task.assignee") }}</legend>
        <div class="task-form__checks">
          <label v-for="member in members" :key="member.userId" class="task-form__check">
            <input
              type="checkbox"
              :data-testid="`task-edit-assignee-${member.userId}`"
              :checked="draftAssigneeIds.includes(member.userId)"
              :disabled="readOnly"
              @change="toggleAssignee(member.userId, draftAssigneeIds.includes(member.userId))"
            />
            <span>{{ formatPersonName(member) }}</span>
          </label>
        </div>
      </fieldset>
      <fieldset class="task-form__field" :disabled="readOnly" data-testid="task-edit-labels">
        <legend>{{ t("task.filter.labelsShort") }}</legend>
        <div class="task-form__checks">
          <p v-if="labels.length === 0" class="task-home__note">{{ t("task.activity.value.none") }}</p>
          <label v-for="label in labels" :key="label.id" class="task-form__check">
            <input
              type="checkbox"
              :data-testid="`task-edit-label-${label.id}`"
              :checked="draftLabelIds.includes(label.id)"
              :disabled="readOnly"
              @change="toggleLabel(label.id, draftLabelIds.includes(label.id))"
            />
            <span>{{ label.name }}</span>
          </label>
        </div>
      </fieldset>
      <div class="task-form__field">
        <label for="task-edit-milestone" class="text-sm font-medium">{{ t("project.milestones") }}</label>
        <select
          id="task-edit-milestone"
          data-testid="task-edit-milestone"
          :aria-label="t('project.milestones')"
          :disabled="readOnly || pending"
          :value="task.milestoneId ?? NONE"
          @change="emit('milestoneChange', ($event.target as HTMLSelectElement).value === NONE ? null : ($event.target as HTMLSelectElement).value)"
        >
          <option :value="NONE">{{ t("task.milestone.none") }}</option>
          <option v-for="milestone in milestones" :key="milestone.id" :value="milestone.id">{{ milestone.name }}</option>
        </select>
      </div>
      <div class="task-form__field" data-testid="task-edit-dependencies">
        <h2 v-if="dependencies.length > 0 || depFormOpen || depLocalError" class="settings-section__title">
          {{ t("task.dep") }}
        </h2>
        <ul v-if="dependencies.length > 0" class="flex flex-col gap-1">
          <li
            v-for="edge in dependencies"
            :key="`${edge.blockerId}-${edge.blockedId}`"
            class="flex items-center justify-between gap-2"
            :data-testid="`task-edit-dependency-${edge.blockerId}-${edge.blockedId}`"
          >
            <span>
              {{ edge.blockerId !== task.id ? "← " : "" }}{{ t("task.dep.lag", { name: dependencyName(edge), type: edge.type, n: edge.lagDays }) }}
            </span>
            <UButton
              v-if="canEdit && !readOnly"
              size="sm"
              variant="outline"
              color="neutral"
              :data-testid="`task-edit-dependency-remove-${edge.blockedId}`"
              :disabled="pending"
              @click="emit('removeDependency', { blockerId: edge.blockerId, blockedId: edge.blockedId })"
            >
              {{ t("task.dependency.remove") }}
            </UButton>
          </li>
        </ul>
        <p v-if="depLocalError" class="task-form__alert" role="alert">{{ depLocalError }}</p>
        <template v-if="canEdit && !readOnly">
          <UButton
            v-if="!depFormOpen"
            size="sm"
            variant="outline"
            color="neutral"
            data-testid="task-edit-dependency-open"
            :disabled="pending"
            @click="depFormOpen = true"
          >
            {{ t("task.dep.add") }}
          </UButton>
          <form v-else class="flex flex-col gap-2" @submit="onAddDependency">
            <select
              v-model="depBlockedId"
              data-testid="task-edit-dependency-target"
              :aria-label="t('task.dep.target')"
              :disabled="pending"
            >
              <option :value="NONE">{{ t("task.dep.targetPlaceholder") }}</option>
              <option
                v-for="candidate in dependencyCandidates.filter((item) => item.id !== task.id)"
                :key="candidate.id"
                :value="candidate.id"
              >
                {{ formatDisplayId(projectKey, candidate.number) }} {{ candidate.title }}
              </option>
            </select>
            <select
              :value="depType"
              data-testid="task-edit-dependency-type"
              :aria-label="t('task.dep.relation')"
              :disabled="pending"
              @change="onDepTypeChange"
            >
              <option v-for="value in DEP_TYPES" :key="value" :value="value">{{ value }}</option>
            </select>
            <input
              v-model.number="depLagDays"
              data-testid="task-edit-dependency-lag"
              :aria-label="t('task.dep.lagDays')"
              type="number"
              min="0"
              step="1"
              :disabled="pending"
            />
            <div class="flex flex-wrap gap-2">
              <UButton type="submit" size="sm" data-testid="task-edit-dependency-add" :disabled="pending">
                {{ t("task.dependency.add") }}
              </UButton>
              <UButton
                size="sm"
                variant="outline"
                color="neutral"
                @click="
                  depFormOpen = false;
                  depLocalError = null;
                "
              >
                {{ t("task.create.cancel") }}
              </UButton>
            </div>
          </form>
        </template>
      </div>
      <p v-if="fieldError" class="task-form__alert" role="alert" data-testid="task-edit-field-error">{{ fieldError }}</p>
      <p v-if="actionError" class="task-form__alert" role="alert" data-testid="task-edit-action-error">{{ actionError }}</p>
      <div v-if="canEdit" class="task-form__actions">
        <UButton
          v-if="!archived"
          size="sm"
          variant="outline"
          color="neutral"
          :disabled="archivePending || pending"
          @click="emit('archiveToggle', true)"
        >
          {{ t("task.archive.action") }}
        </UButton>
        <UButton
          size="sm"
          variant="outline"
          color="neutral"
          data-testid="task-edit-trash"
          :disabled="trashPending || pending"
          @click="emit('trash')"
        >
          {{ t("task.trash.action") }}
        </UButton>
      </div>
    </div>
    <p class="task-home__note tabular-nums">{{ displayId }}</p>
  </div>
</template>
