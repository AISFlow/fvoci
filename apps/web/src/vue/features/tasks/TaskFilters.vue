<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { computed, ref, useId, watch } from "vue";
import type { WorkflowStatus } from "@/features/projects/queries";
import type { LabelItem, MilestoneItem } from "@/features/tasks/queries";
import { PRIORITIES, priorityLabel } from "@/features/tasks/task-edit-payload";
import { TASK_TYPES, TASK_TYPE_LABELS } from "@/features/tasks/task-types";
import { SORTABLE_FIELD_TYPES } from "@/lib/collection-values";
import type { MemberOutput } from "@/lib/contracts";
import type { CollectionField } from "@/lib/queries/collections";
import {
  isEmptyViewQuery,
  patchViewFilter,
  readPrimarySort,
  setPrimarySort,
  type ViewFilters,
  type ViewQuery,
} from "@/lib/view-query";
import CustomFilters from "../collections/CustomFilters.vue";
import "@/features/collections/collections.css";

type SelectKey = "type" | "statusId" | "priority" | "assigneeId" | "labelId" | "milestoneId";

// The project task list's filter and sort bar over the shared view query
// (features/tasks/task-filters.tsx). Every change goes to the URL (the
// page's `?query=`); the title applies on Enter or when the box loses focus.
const props = defineProps<{
  query: ViewQuery;
  statuses: readonly WorkflowStatus[];
  labels: readonly LabelItem[];
  milestones: readonly MilestoneItem[];
  members: readonly MemberOutput[];
  fields: readonly CollectionField[];
  timeZone: string;
}>();
const emit = defineEmits<{ change: [next: ViewQuery] }>();

const id = useId();
const filters = computed<ViewFilters>(() => props.query.filters);
const title = ref(filters.value.title ?? "");
const customOpen = ref((filters.value.custom?.length ?? 0) > 0);
watch(
  () => filters.value.title,
  (next) => {
    title.value = next ?? "";
  },
);

const activeFields = computed(() => props.fields.filter((field) => field.deletedAt === null));
const sortItems = computed(() => [
  { id: "created", name: t("collection.created") },
  { id: "title", name: t("collection.resourceTitle") },
  { id: "status", name: t("collection.status") },
  { id: "due", name: t("collection.due") },
  { id: "priority", name: t("task.priority") },
  ...activeFields.value
    .filter((field) => SORTABLE_FIELD_TYPES.includes(field.type))
    .map((field) => ({ id: field.id, name: field.name })),
]);
const simple = computed(() => {
  const primary = readPrimarySort(props.query);
  return primary && sortItems.value.some((item) => item.id === primary.field) ? primary : null;
});
const sortValue = computed(
  () => simple.value?.field ?? (props.query.sort.length > 0 ? "advanced" : "default"),
);

const selects = computed<
  { key: SelectKey; label: string; items: { id: string; name: string }[] }[]
>(() => [
  {
    key: "type",
    label: t("task.filter.typeShort"),
    items: TASK_TYPES.map((type) => ({ id: type, name: TASK_TYPE_LABELS[type] })),
  },
  {
    key: "statusId",
    label: t("task.filter.statusShort"),
    items: props.statuses.map((status) => ({ id: status.id, name: status.name })),
  },
  {
    key: "priority",
    label: t("task.filter.priorityShort"),
    items: PRIORITIES.map((priority) => ({ id: priority, name: priorityLabel(priority) })),
  },
  {
    key: "assigneeId",
    label: t("task.filter.assigneeShort"),
    items: [
      { id: "me", name: t("task.filter.me") },
      ...props.members.map((member) => ({ id: member.userId, name: formatPersonName(member) })),
    ],
  },
  ...(props.labels.length > 0
    ? [
        {
          key: "labelId" as const,
          label: t("task.filter.labelsShort"),
          items: props.labels.map((label) => ({ id: label.id, name: label.name })),
        },
      ]
    : []),
  ...(props.milestones.length > 0
    ? [
        {
          key: "milestoneId" as const,
          label: t("task.filter.milestoneShort"),
          items: props.milestones.map((milestone) => ({ id: milestone.id, name: milestone.name })),
        },
      ]
    : []),
]);

function applyTitle(): void {
  emit("change", patchViewFilter(props.query, "title", title.value));
}

function onTitleBlur(): void {
  if (title.value.trim() !== (filters.value.title ?? "")) applyTitle();
}

function onSelect(key: SelectKey, event: Event): void {
  emit(
    "change",
    patchViewFilter(props.query, key, (event.target as HTMLSelectElement).value || undefined),
  );
}

function onSort(event: Event): void {
  const value = (event.target as HTMLSelectElement).value;
  emit(
    "change",
    setPrimarySort(
      props.query,
      value === "default" || value === "advanced" ? null : value,
      simple.value?.direction ?? "asc",
    ),
  );
}

function toggleDirection(): void {
  const current = simple.value;
  if (!current) return;
  emit(
    "change",
    setPrimarySort(props.query, current.field, current.direction === "asc" ? "desc" : "asc"),
  );
}
</script>

<template>
  <div class="flex flex-col gap-2" role="search" :aria-label="t('view.filter')">
    <div class="collection-toolbar">
      <form class="collection-field" @submit.prevent="applyTitle">
        <label :for="`${id}-title`">{{ t("gantt.search") }}</label>
        <!-- :value + @input, not v-model: the draft follows Korean composition as typed. -->
        <input
          :id="`${id}-title`"
          class="h-9 w-48 rounded-md border border-default bg-default px-3 text-sm"
          type="search"
          data-testid="task-filter-title"
          :value="title"
          maxlength="1000"
          @input="title = ($event.target as HTMLInputElement).value"
          @blur="onTitleBlur"
        />
      </form>
      <div v-for="select in selects" :key="select.key" class="collection-field">
        <label :for="`${id}-${select.key}`">{{ select.label }}</label>
        <select
          :id="`${id}-${select.key}`"
          class="collection-select"
          :data-testid="`task-filter-${select.key}`"
          :value="filters[select.key] ?? ''"
          @change="onSelect(select.key, $event)"
        >
          <option value="">{{ t("task.filter.all") }}</option>
          <option v-for="item in select.items" :key="item.id" :value="item.id">{{
            item.name
          }}</option>
        </select>
      </div>
      <div class="collection-field">
        <label :for="`${id}-due`">{{ t("task.filter.dueBefore") }}</label>
        <input
          :id="`${id}-due`"
          class="h-9 rounded-md border border-default bg-default px-3 text-sm"
          type="date"
          :value="filters.dueBefore ?? ''"
          @input="
            emit(
              'change',
              patchViewFilter(
                query,
                'dueBefore',
                ($event.target as HTMLInputElement).value || undefined,
              ),
            )
          "
        />
      </div>
      <label class="flex min-h-9 items-center gap-2 text-sm" :for="`${id}-open`">
        <input
          :id="`${id}-open`"
          type="checkbox"
          data-testid="task-filter-openOnly"
          :checked="filters.openOnly === true"
          @change="
            emit(
              'change',
              patchViewFilter(query, 'openOnly', ($event.target as HTMLInputElement).checked),
            )
          "
        />
        {{ t("task.filter.openOnly") }}
      </label>
    </div>
    <div class="collection-toolbar">
      <div class="collection-field">
        <label :for="`${id}-sort`">{{ t("collection.sort") }}</label>
        <select
          :id="`${id}-sort`"
          class="collection-select"
          data-testid="task-sort"
          :value="sortValue"
          @change="onSort"
        >
          <option value="default">{{ t("collection.sort.default") }}</option>
          <option v-if="sortValue === 'advanced'" value="advanced">{{
            t("collection.sort.advanced")
          }}</option>
          <option v-for="item in sortItems" :key="item.id" :value="item.id">{{ item.name }}</option>
        </select>
      </div>
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="!simple"
        @click="toggleDirection"
      >
        {{ simple?.direction === "desc" ? t("collection.descending") : t("collection.ascending") }}
      </UButton>
      <UButton
        v-if="activeFields.length > 0"
        size="sm"
        variant="outline"
        color="neutral"
        :aria-expanded="customOpen"
        @click="customOpen = !customOpen"
      >
        {{ t("collection.filter.custom") }}
      </UButton>
      <UButton
        v-if="!isEmptyViewQuery(query)"
        size="sm"
        variant="outline"
        color="neutral"
        @click="emit('change', { filters: {}, sort: [] })"
      >
        {{ t("task.filter.clear") }}
      </UButton>
    </div>
    <CustomFilters
      v-if="customOpen && activeFields.length > 0"
      :fields="activeFields"
      :members="members"
      :time-zone="timeZone"
      :query="query"
      @change="emit('change', $event)"
    />
  </div>
</template>
