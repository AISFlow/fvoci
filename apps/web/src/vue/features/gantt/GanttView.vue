<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UAlert from "@nuxt/ui/components/Alert.vue";
import UButton from "@nuxt/ui/components/Button.vue";
import UInput from "@nuxt/ui/components/Input.vue";
import { useQuery } from "@tanstack/vue-query";
import { computed, onBeforeUnmount, ref, watch } from "vue";
import { workflowQuery } from "@/features/projects/queries";
import { formatDisplayId } from "@/lib/href";
import type { IsoDate } from "@/lib/iso-date";
import { problemMessage } from "@/lib/api";
import { membersQuery } from "@/lib/queries";
import { EMPTY_VIEW_QUERY, withTitleFilter, type ViewQuery } from "@/lib/view-query";
import GanttChart, { type GanttBarChange } from "./GanttChart.vue";
import type { PackMode } from "./gantt-geometry";
import { useGanttLayout } from "./useGanttLayout";
import { useRescheduleTask } from "./useRescheduleTask";
import "./gantt-theme.css";

const props = defineProps<{
  workspaceId: string;
  projectId: string;
  projectKey: string;
  year: number;
  month: number;
  query: ViewQuery;
  weekStartsOn: 0 | 1;
  timeZone: string;
  today: IsoDate;
}>();

const emit = defineEmits<{
  /** Show the previous (-1) or next (+1) month. */
  shiftMonth: [delta: -1 | 1];
  /** The title filter, as typed. */
  search: [title: string];
  /** Open the task (a page of the React app). */
  openTask: [displayId: string];
}>();

const layoutQuery = useGanttLayout(() => ({
  workspaceId: props.workspaceId,
  projectId: props.projectId,
  filters: { year: props.year, month: props.month, weekStartsOn: props.weekStartsOn, query: props.query },
}));
const workflow = useQuery(() => workflowQuery(props.workspaceId, props.projectId));
const members = useQuery(() => membersQuery(props.workspaceId));
const reschedule = useRescheduleTask(() => ({
  workspaceId: props.workspaceId,
  projectId: props.projectId,
  timeZone: props.timeZone,
}));

const pack = ref<PackMode>("rows");
const layout = computed(() => layoutQuery.data.value);
const items = computed(() => layout.value?.items ?? []);
const itemsById = computed(() => new Map(items.value.map((item) => [item.id, item])));
const statusById = computed(() => new Map((workflow.data.value?.statuses ?? []).map((s) => [s.id, s])));
const memberNameById = computed(
  () => new Map((members.data.value?.items ?? []).map((m) => [m.userId, formatPersonName(m)])),
);
// A month or filter still loading shows the previous layout; it takes no edits.
const canEdit = computed(() => layout.value?.canEdit === true && !layoutQuery.isPlaceholderData.value);

function assigneeName(ids: readonly string[]): string | undefined {
  return ids.map((id) => memberNameById.value.get(id)).find((name): name is string => name !== undefined);
}

function onChange(change: GanttBarChange): void {
  const item = itemsById.value.get(change.id);
  if (!item || !canEdit.value || reschedule.savingId.value !== null) return;
  reschedule.reschedule({ id: change.id, item, change });
}

function onSelect(id: string): void {
  const item = itemsById.value.get(id);
  if (item) emit("openTask", formatDisplayId(props.projectKey, item.number));
}

// The search box writes the title filter to the URL a moment after typing
// stops, so each keystroke does not load a layout. A URL change that is the
// echo of our own search is not written back into the box: the user may have
// typed more since, and those characters would be lost.
const title = ref(props.query.filters.title ?? "");
/** Title filters sent with `search` that the URL has not shown yet, oldest first. */
const echoes: string[] = [];
watch(
  () => props.query.filters.title ?? "",
  (next) => {
    const echo = echoes.indexOf(next);
    if (echo >= 0) {
      echoes.splice(0, echo + 1);
      return;
    }
    echoes.length = 0;
    if (next !== title.value.trim()) title.value = next;
  },
);
let searchTimer: number | undefined;
function onTitleInput(value: string | number | null | undefined): void {
  title.value = String(value ?? "");
  window.clearTimeout(searchTimer);
  searchTimer = window.setTimeout(() => {
    // The filter as the URL will hold it (trimmed, capped); an unchanged one
    // changes no URL and has no echo.
    const next = withTitleFilter(EMPTY_VIEW_QUERY, title.value).filters.title ?? "";
    if (next !== (props.query.filters.title ?? "")) echoes.push(next);
    emit("search", title.value);
  }, 300);
}
onBeforeUnmount(() => window.clearTimeout(searchTimer));
</script>

<template>
  <div class="flex flex-col gap-3">
    <div
      class="flex flex-wrap items-center gap-2 rounded-md border border-default bg-elevated/40 p-3"
      data-slot="gantt-toolbar"
    >
      <div class="flex flex-wrap items-center gap-2">
        <UButton color="neutral" variant="outline" size="sm" @click="emit('shiftMonth', -1)">
          {{ t("cal.prevMonth") }}
        </UButton>
        <p class="min-w-24 text-center tabular-nums" data-slot="gantt-period">
          {{ t("cal.yearMonth", { year, month }) }}
        </p>
        <UButton color="neutral" variant="outline" size="sm" @click="emit('shiftMonth', 1)">
          {{ t("cal.nextMonth") }}
        </UButton>
      </div>
      <UButton
        color="neutral"
        :variant="pack === 'overlap' ? 'solid' : 'outline'"
        size="sm"
        :aria-pressed="pack === 'overlap'"
        @click="pack = pack === 'overlap' ? 'rows' : 'overlap'"
      >
        {{ t("gantt.flow") }}
      </UButton>
      <UInput
        class="min-w-0 flex-[1_1_12rem] sm:max-w-56"
        type="search"
        icon="i-lucide-search"
        :model-value="title"
        :placeholder="t('gantt.search')"
        :aria-label="t('gantt.search')"
        :maxlength="200"
        @update:model-value="onTitleInput"
      />
    </div>
    <UAlert
      v-if="reschedule.error.value"
      role="alert"
      color="error"
      variant="subtle"
      :title="reschedule.error.value"
      close
      @update:open="reschedule.dismissError()"
    />
    <p role="status" class="sr-only">{{ reschedule.savingId.value ? t("gantt.bar.saving") : "" }}</p>
    <p v-if="layout?.truncated" role="status" class="text-muted">{{ t("gantt.truncated") }}</p>
    <p v-if="layout && layout.linkTotal > layout.links.length" role="status" class="text-muted">
      {{ t("gantt.paths.truncated", { shown: layout.links.length, total: layout.linkTotal }) }}
    </p>
    <p v-if="layoutQuery.isPending.value" role="status" class="p-4 text-muted">{{ t("load.loading") }}</p>
    <div v-else-if="layoutQuery.isError.value && !layout" role="alert" class="flex items-center gap-2 p-4">
      <span>{{ problemMessage(layoutQuery.error.value, "load.failed") }}</span>
      <UButton size="sm" variant="outline" color="neutral" @click="layoutQuery.refetch()">
        {{ t("load.retry") }}
      </UButton>
    </div>
    <p v-else-if="!layout || items.length === 0" class="p-4 text-muted" data-slot="gantt-empty">
      {{ t("task.view.empty") }}
    </p>
    <GanttChart
      v-else
      :layout="layout"
      :can-edit="canEdit"
      :saving-id="reschedule.savingId.value"
      :pending="reschedule.pending.value"
      :today="today"
      :pack="pack"
      @select="onSelect"
      @change="onChange"
    >
      <template #rail-row="{ item }">
        <span class="flex min-w-0 items-center gap-1">
          <span
            v-if="statusById.get(item.statusId)"
            class="shrink-0 text-xs text-muted"
            :title="statusById.get(item.statusId)?.name"
            >{{ statusById.get(item.statusId)?.name }}</span
          >
          <span class="shrink-0 font-mono text-xs text-muted">{{ formatDisplayId(projectKey, item.number) }}</span>
          <span class="min-w-0 flex-1 truncate">{{ item.title }}</span>
          <span v-if="assigneeName(item.assigneeIds)" class="max-w-16 shrink-0 truncate text-xs text-muted">{{
            assigneeName(item.assigneeIds)
          }}</span>
        </span>
      </template>
    </GanttChart>
  </div>
</template>
