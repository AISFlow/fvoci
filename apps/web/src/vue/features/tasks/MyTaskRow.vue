<script setup lang="ts">
import { computed } from "vue";
import { t } from "@fvoci/i18n";
import UAvatar from "@nuxt/ui/components/Avatar.vue";
import UBadge from "@nuxt/ui/components/Badge.vue";
import type { components } from "@/generated/api";
import { priorityLabel } from "@/features/tasks/task-edit-payload";
import { taskTypeLabel } from "@/features/tasks/task-types";
import AppLink from "../../components/AppLink.vue";
import { taskDueLabel } from "./task-due";
import { formatDisplayId, itemPath } from "@/lib/href";

const props = defineProps<{
  slug: string;
  itemId: string;
  title: string;
  number: number;
  projectKey?: string;
  statusName?: string;
  dueDate?: string | null;
  dueAt?: string | null;
  type?: string;
  priority?: string;
  labels?: readonly components["schemas"]["LabelOutput"][];
  assigneeIds?: readonly string[];
  members?: readonly components["schemas"]["MemberResponse"][];
  timeZone: string;
}>();

const href = computed(() =>
  props.projectKey ? itemPath(props.slug, formatDisplayId(props.projectKey, props.number)) : null,
);
const due = computed(() => taskDueLabel(props.dueDate, props.dueAt, props.timeZone));
const ownLabels = computed(() => props.labels ?? []);
const assignees = computed(() => (props.assigneeIds ?? []).map((id) => props.members?.find((member) => member.userId === id)));
const memberName = (member: components["schemas"]["MemberResponse"] | undefined) => member
  ? [member.familyName, member.givenName].filter(Boolean).join("") : t("task.assignee");
const displayId = computed(() =>
  props.projectKey ? formatDisplayId(props.projectKey, props.number) : props.itemId.slice(0, 8),
);
</script>

<template>
  <component :is="href ? AppLink : 'span'" :to="href ?? ''" class="task-row flex flex-wrap items-center gap-2" :data-testid="`my-task-${itemId}`">
    <span class="task-row__id">{{ displayId }}</span>
    <span v-if="type" class="text-xs text-muted" :aria-label="taskTypeLabel(type)">{{ taskTypeLabel(type) }}</span>
    <span class="task-row__title">{{ title }}</span>
    <UBadge v-if="statusName" color="neutral" variant="subtle" size="sm">{{ statusName }}</UBadge>
    <span class="flex items-center gap-1" :aria-label="t('task.filter.labelsShort')">
      <UBadge v-for="label in ownLabels.slice(0, 2)" :key="label.id" color="neutral" variant="outline" size="sm">
        <span class="size-2 rounded-full" :style="{ backgroundColor: label.color }" aria-hidden="true" />{{ label.name }}
      </UBadge>
      <span v-if="ownLabels.length > 2" class="text-xs text-muted">+{{ ownLabels.length - 2 }}</span>
    </span>
    <span v-if="priority && priority !== 'none'" class="text-xs text-muted" :aria-label="`${t('task.priority')}: ${priorityLabel(priority)}`">{{ priorityLabel(priority) }}</span>
    <span v-if="due" class="project-list__private">{{ due }}</span>
    <span class="flex -space-x-1" :aria-label="t('task.assignee')">
      <UAvatar v-for="(member, index) in assignees.slice(0, 3)" :key="assigneeIds?.[index]" :alt="memberName(member)" :title="memberName(member)" size="2xs" />
      <span v-if="assignees.length > 3" class="text-xs text-muted" :aria-label="t('task.assignees.additional', { count: assignees.length - 3 })">+{{ assignees.length - 3 }}</span>
    </span>
  </component>
</template>
