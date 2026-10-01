<script setup lang="ts">
import { formatPersonName, t } from "@fvoci/i18n";
import UButton from "@nuxt/ui/components/Button.vue";
import { useMutation, useQuery, useQueryClient } from "@tanstack/vue-query";
import { computed, ref, useId, watch } from "vue";
import type { BoardGroup } from "@/features/collections/board-model";
import { moveRequest } from "@/features/collections/board-model";
import {
  CALENDAR_DRAG_TYPE,
  dateMovable,
  dateMoveRequest,
  type CalendarRow,
} from "@/features/collections/calendar-model";
import { settleTaskPatch } from "@/features/tasks/task-patch-cache";
import {
  collectionConfigOf,
  defaultConfig,
  isViewType,
  PAGE_LIMIT,
  type CollectionViewType,
} from "@/features/collections/collection-view";
import {
  asCollectionValue,
  formatCollectionValue,
  isMonth,
  monthWindow,
  SORTABLE_FIELD_TYPES,
  todayInTimeZone,
  type CollectionValue,
} from "@/lib/collection-values";
import type { MemberOutput } from "@/lib/contracts";
import { FALLBACK_TZ } from "@/lib/datetime";
import { itemPath } from "@/lib/href";
import {
  api,
  ensureOk,
  isInvalidCursor,
  isInvalidInput,
  loadErrorMessage,
  ProblemError,
  problemMessage,
} from "@/lib/api";
import { membersQuery, meQuery } from "@/lib/queries";
import {
  asJsonObject,
  collectionFieldsQuery,
  collectionPrefix,
  collectionRowsQuery,
  collectionViewsQuery,
  putCollectionValue,
  type CollectionConfig,
  type CollectionField,
  type CollectionQueryBody,
  type CollectionQueryItem,
  type CollectionQueryPreview,
  type CollectionView,
} from "@/lib/queries/collections";
import { readPrimarySort, setPrimarySort } from "@/lib/view-query";
import ConfirmActionButton from "../../components/ConfirmActionButton.vue";
import QueryError from "../../components/QueryError.vue";
import QueryLoading from "../../components/QueryLoading.vue";
import CollectionBoard from "./CollectionBoard.vue";
import CollectionCalendar from "./calendar/CollectionCalendar.vue";
import { optimisticRow, type CalendarWrite } from "./calendar/calendar-adapter";
import CustomFilters from "./CustomFilters.vue";
import ValueEditor from "./ValueEditor.vue";
import "@/features/collections/collections.css";

const props = defineProps<{
  workspaceId: string;
  slug: string;
  collectionId: string;
  projectId: string;
  type: CollectionViewType;
  initialViewId: string | null;
}>();
const emit = defineEmits<{ openView: [type: CollectionViewType, viewId: string | null] }>();

const queryClient = useQueryClient();
const baseId = useId();
const prefix = computed(() => collectionPrefix(props.workspaceId, props.collectionId));
const fields = useQuery(() => collectionFieldsQuery(props.workspaceId, props.collectionId));
const views = useQuery(() => collectionViewsQuery(props.workspaceId, props.collectionId));
const members = useQuery(() => membersQuery(props.workspaceId));
const me = useQuery(() => meQuery);
const timeZone = computed(() => me.data.value?.timezone ?? FALLBACK_TZ);
const weekStartsOn = computed(() => (me.data.value?.weekStartsOn === 0 ? 0 : 1));

const config = ref<CollectionConfig>(defaultConfig(props.type));
const view = ref<CollectionView | null>(null);
const viewName = ref("");
const visibility = ref<"private" | "shared">("private");
const viewConflict = ref(false);
const cursor = ref<string | undefined>();
const day = ref<string | null | undefined>();
const month = ref("");
const visibleRange = ref<{ from: string; to: string }>();
const pendingPreview = ref<CollectionQueryPreview | null>(null);
const customOpen = ref(false);
const moveError = ref<string | null>(null);
const moving = ref(false);
const draggedDate = ref<CalendarRow | null>(null);
const calendarUI = ref<{
  nativeStart(row: CollectionQueryPreview): void;
  pointerdown(event: PointerEvent, row: CollectionQueryPreview): void;
  cancel(): void;
}>();
const dropDay = ref<string | null | undefined>(undefined);

const effectiveMonth = computed(() =>
  isMonth(month.value) ? month.value : todayInTimeZone(timeZone.value).slice(0, 7),
);

function applyView(saved: CollectionView | null): void {
  view.value = saved;
  viewConflict.value = false;
  cursor.value = undefined;
  day.value = undefined;
  if (saved) {
    config.value = collectionConfigOf(saved);
    viewName.value = saved.name;
    visibility.value = saved.visibility === "shared" ? "shared" : "private";
  } else {
    config.value = defaultConfig(props.type);
    viewName.value = "";
    visibility.value = "private";
  }
}

watch(
  () => [views.data.value, props.initialViewId] as const,
  () => {
    const list = views.data.value;
    if (!list || (props.initialViewId ?? null) === (view.value?.id ?? null)) return;
    if (!props.initialViewId) {
      applyView(null);
      return;
    }
    const saved = list.items.find((item) => item.id === props.initialViewId);
    if (saved) applyView(saved);
  },
  { immediate: true },
);

const groupedBoard = computed(() => props.type === "board" && config.value.groupBy !== null);
const calendarWindow = computed(() =>
  props.type === "calendar" && config.value.dateBy
    ? { ...(visibleRange.value ?? monthWindow(effectiveMonth.value)), timeZone: timeZone.value }
    : undefined,
);
const queryBody = computed<CollectionQueryBody>(() => ({
  config: config.value,
  limit: groupedBoard.value ? 1 : PAGE_LIMIT,
  ...(cursor.value && !groupedBoard.value ? { cursor: cursor.value } : {}),
  ...(props.type === "calendar" && config.value.dateBy && day.value !== undefined
    ? { day: day.value }
    : {}),
  ...(calendarWindow.value ? { window: calendarWindow.value } : {}),
}));
// Keep the same Calendar draft through a failed background fields load. HTTP
// denials and unexpected errors still retire it; cached fields are not an ACL.
const fieldsRefreshFailed = computed(() => {
  const error = fields.error.value;
  return (
    props.type === "calendar" &&
    fields.data.value !== undefined &&
    fields.isError.value &&
    (error instanceof TypeError ||
      (error instanceof ProblemError && [408, 429, 500, 502, 503, 504].includes(error.status)))
  );
});
const rowsEnabled = computed(
  () =>
    (fields.isSuccess.value || fieldsRefreshFailed.value) &&
    views.isSuccess.value &&
    me.isSuccess.value,
);
const rows = useQuery(() =>
  collectionRowsQuery(props.workspaceId, props.collectionId, queryBody.value, rowsEnabled.value),
);

watch(
  () => rows.error.value,
  (err) => {
    if (cursor.value && isInvalidCursor(err)) cursor.value = undefined;
  },
);

async function refresh(): Promise<void> {
  await queryClient.invalidateQueries({ queryKey: prefix.value });
}

function change(patch: Partial<CollectionConfig>): void {
  config.value = { ...config.value, ...patch };
  cursor.value = undefined;
  day.value = undefined;
}

function valueOf(row: { values: Record<string, never> }, fieldId: string): unknown {
  return (row.values as Record<string, unknown>)[fieldId];
}

async function setValue(
  row: CollectionQueryItem,
  field: CollectionField,
  value: CollectionValue,
): Promise<void> {
  try {
    await putCollectionValue(props.workspaceId, props.collectionId, row.id, {
      fieldId: field.id,
      expectedVersion: row.version,
      expectedFieldVersion: field.version,
      value,
    });
  } finally {
    await refresh();
  }
}

async function moveToGroup(row: CollectionQueryItem, target: BoardGroup): Promise<void> {
  const request = moveRequest(config.value.groupBy, row, target);
  if (!request || moving.value) return;
  moveError.value = null;
  moving.value = true;
  try {
    if (request.kind === "status") {
      await ensureOk(
        await api.POST("/api/v1/workspaces/{workspace_id}/tasks/{task_id}/move", {
          params: { path: { workspace_id: props.workspaceId, task_id: request.taskId } },
          body: { statusId: request.statusId, expectedStatusId: request.expectedStatusId },
        }),
      );
      await queryClient.invalidateQueries({
        queryKey: ["tasks", props.workspaceId, props.projectId],
      });
    } else {
      const field = fields.data.value?.items.find((item) => item.id === request.fieldId);
      if (!field) return;
      await putCollectionValue(props.workspaceId, props.collectionId, row.id, {
        fieldId: field.id,
        expectedVersion: row.version,
        expectedFieldVersion: field.version,
        value: request.value,
      });
    }
  } catch (err) {
    moveError.value = problemMessage(err, "collection.saveError");
  } finally {
    await refresh();
    moving.value = false;
  }
}

async function moveToDate(row: CalendarRow, target: string | null): Promise<void> {
  const request = dateMoveRequest(
    config.value.dateBy,
    row,
    target,
    fields.data.value?.items ?? [],
    timeZone.value,
  );
  if (!request || moving.value) return;
  moveError.value = null;
  if (request.kind === "unavailable") {
    moveError.value = t("collection.saveError");
    return;
  }
  const preview = (rows.data.value?.previews ?? []).find((item) => item.id === row.id);
  try {
    await saveCalendarDate(
      preview ?? { ...row, displayId: "", title: "", documentId: null, statusId: null },
      request,
    );
  } catch {
    /* moveError is shown */
  }
}

async function saveCalendarDate(
  row: CollectionQueryPreview,
  request: CalendarWrite,
): Promise<void> {
  if (moving.value || !navigator.onLine || !dateMovable(config.value.dateBy, row, active.value))
    throw new Error("Calendar write unavailable");
  moveError.value = null;
  moving.value = true;
  const preview = optimisticRow(row, request, timeZone.value, config.value.dateBy ?? undefined);
  pendingPreview.value = preview;
  try {
    if (request.kind === "task") {
      const accepted = await ensureOk(
        await api.PATCH("/api/v1/workspaces/{workspace_id}/tasks/{task_id}", {
          params: { path: { workspace_id: props.workspaceId, task_id: request.taskId } },
          body: request.body,
        }),
      );
      // Use accepted stored dates, never treat the requested values as server truth.
      pendingPreview.value = optimisticRow(
        row,
        {
          ...request,
          body: {
            ...request.body,
            ...(request.body.startDate !== undefined ? { startDate: accepted.startDate } : {}),
            ...(request.body.dueDate !== undefined ? { dueDate: accepted.dueDate } : {}),
            ...(request.body.dueAt !== undefined ? { dueAt: accepted.dueAt } : {}),
          },
        },
        timeZone.value,
        config.value.dateBy ?? undefined,
      );
      await settleTaskPatch(queryClient, accepted.workspaceId, accepted.projectId, accepted);
    } else {
      const accepted = await putCollectionValue(props.workspaceId, props.collectionId, row.id, {
        fieldId: request.fieldId,
        expectedVersion: request.expectedVersion,
        expectedFieldVersion: request.expectedFieldVersion,
        value: request.value,
      });
      pendingPreview.value = { ...preview, version: accepted.version };
    }
  } catch (err) {
    pendingPreview.value = null;
    moveError.value = problemMessage(err, "collection.saveError");
    throw err;
  } finally {
    try {
      await refresh();
    } finally {
      pendingPreview.value = null;
      moving.value = false;
    }
  }
}
function calendarTarget(event: DragEvent): string | null | undefined {
  const cell = (event.target as HTMLElement).closest<HTMLElement>("[data-calendar-target]");
  return cell ? cell.dataset.calendarTarget || null : undefined;
}
function externalCalendarOver(event: DragEvent) {
  const target = calendarTarget(event);
  if (target !== undefined) onDateDragOver(event, target);
}
async function externalCalendarDrop(event: DragEvent) {
  const target = calendarTarget(event);
  if (target !== undefined) await onDateDrop(event, target);
}

const saveView = useMutation({
  mutationFn: async () => {
    const payload = {
      name: viewName.value.trim(),
      type: props.type,
      visibility: visibility.value,
      config: asJsonObject(config.value),
    };
    if (view.value) {
      return ensureOk(
        await api.PATCH(
          "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views/{view_id}",
          {
            params: {
              path: {
                workspace_id: props.workspaceId,
                collection_id: props.collectionId,
                view_id: view.value.id,
              },
            },
            body: { ...payload, expectedVersion: view.value.version },
          },
        ),
      );
    }
    return ensureOk(
      await api.POST("/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views", {
        params: { path: { workspace_id: props.workspaceId, collection_id: props.collectionId } },
        body: payload,
      }),
    );
  },
  onError: async (err) => {
    if (err instanceof ProblemError && err.status === 409) {
      viewConflict.value = true;
      await views.refetch();
    }
  },
  onSuccess: async (saved) => {
    view.value = saved;
    emit("openView", props.type, saved.id);
    await refresh();
  },
});

const removeView = useMutation({
  mutationFn: async (viewId: string) =>
    ensureOk(
      await api.DELETE(
        "/api/v1/workspaces/{workspace_id}/collections/{collection_id}/views/{view_id}",
        {
          params: {
            path: {
              workspace_id: props.workspaceId,
              collection_id: props.collectionId,
              view_id: viewId,
            },
          },
        },
      ),
    ),
  onSuccess: async () => {
    applyView(null);
    emit("openView", props.type, null);
    await refresh();
  },
});

const failed = computed(
  () =>
    (fields.isError.value && !fieldsRefreshFailed.value) || views.isError.value || me.isError.value,
);
const memberItems = computed<MemberOutput[]>(() => members.data.value?.items ?? []);
const userNames = computed(() =>
  memberItems.value.map((member) => ({ userId: member.userId, name: formatPersonName(member) })),
);
const active = computed(() =>
  (fields.data.value?.items ?? []).filter((field) => field.deletedAt === null),
);
const canSave = computed(() => views.data.value?.canSave === true);
const canManageViews = computed(() => views.data.value?.canManage === true);
const ownsView = computed(
  () => view.value === null || view.value.ownerId === me.data.value?.userId,
);
const shareBlocked = computed(
  () =>
    (visibility.value === "shared" || view.value?.visibility === "shared") && !canManageViews.value,
);
const invalidQuery = computed(() => isInvalidInput(rows.error.value));
const sortItems = computed(() => [
  { id: "created", name: t("collection.created") },
  { id: "title", name: t("collection.resourceTitle") },
  ...active.value
    .filter((field) => SORTABLE_FIELD_TYPES.includes(field.type))
    .map((field) => ({ id: field.id, name: field.name })),
]);
const primary = computed(() => readPrimarySort(config.value.query));
const simple = computed(() =>
  primary.value && sortItems.value.some((item) => item.id === primary.value?.field)
    ? primary.value
    : null,
);
const sortValue = computed(
  () => simple.value?.field ?? (config.value.query.sort.length > 0 ? "advanced" : "default"),
);
const calendarDrag = computed(() => props.type === "calendar" && config.value.dateBy !== null);
const calendarPreviews = computed(() => {
  const list = rows.data.value?.previews ?? [];
  const preview = pendingPreview.value;
  return preview ? [...list.filter((row) => row.id !== preview.id), preview] : list;
});
const dayCounts = computed(() => {
  const counts = new Map((rows.data.value?.days ?? []).map((entry) => [entry.date, entry.count]));
  const original = rows.data.value?.previews.find((row) => row.id === pendingPreview.value?.id);
  if (original && pendingPreview.value && original.date !== pendingPreview.value.date) {
    counts.set(original.date, Math.max(0, (counts.get(original.date) ?? 0) - 1));
    counts.set(pendingPreview.value.date, (counts.get(pendingPreview.value.date) ?? 0) + 1);
  }
  return counts;
});
const groups = computed(() => rows.data.value?.groups ?? []);

function formatValue(field: CollectionField, raw: unknown): string {
  return formatCollectionValue(
    asCollectionValue(raw),
    field.options,
    userNames.value,
    timeZone.value,
    {
      yes: t("collection.filter.true"),
      no: t("collection.filter.false"),
    },
  );
}

function canMoveDate(row: CalendarRow | null, target: string | null): boolean {
  return (
    !moving.value &&
    row !== null &&
    dateMoveRequest(config.value.dateBy, row, target, active.value, timeZone.value) !== null
  );
}

function onDateDragStart(event: DragEvent, row: CollectionQueryItem): void {
  if (!calendarDrag.value || moving.value || !dateMovable(config.value.dateBy, row, active.value))
    return;
  event.dataTransfer?.setData(CALENDAR_DRAG_TYPE, row.id);
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
  draggedDate.value = row;
  calendarUI.value?.nativeStart(row);
}

function onDateDragEnd(): void {
  calendarUI.value?.cancel();
  draggedDate.value = null;
  dropDay.value = undefined;
}

function onDateDragOver(event: DragEvent, target: string | null): void {
  if (!canMoveDate(draggedDate.value, target)) return;
  event.preventDefault();
  if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
  if (dropDay.value !== target) dropDay.value = target;
}

function onDateDragLeave(event: DragEvent): void {
  const current = event.currentTarget as Node | null;
  if (current && !current.contains(event.relatedTarget as Node | null)) dropDay.value = undefined;
}

async function onDateDrop(event: DragEvent, target: string | null): Promise<void> {
  const row = draggedDate.value;
  draggedDate.value = null;
  dropDay.value = undefined;
  if (!row || !canMoveDate(row, target)) return;
  if (event.dataTransfer?.getData(CALENDAR_DRAG_TYPE) !== row.id) return;
  event.preventDefault();
  await moveToDate(row, target);
}

function dateDraggable(row: CalendarRow): boolean {
  return calendarDrag.value && !moving.value && dateMovable(config.value.dateBy, row, active.value);
}

function onSavedViewChange(event: Event): void {
  const saved =
    views.data.value?.items.find((item) => item.id === (event.target as HTMLSelectElement).value) ??
    null;
  saveView.reset();
  if (saved && isViewType(saved.type) && saved.type !== props.type) {
    emit("openView", saved.type, saved.id);
    return;
  }
  applyView(saved);
  emit("openView", props.type, saved?.id ?? null);
}

function onGroupBy(event: Event): void {
  change({ groupBy: (event.target as HTMLSelectElement).value || null });
}

function onDateBy(event: Event): void {
  change({ dateBy: (event.target as HTMLSelectElement).value || null });
}

function onSort(event: Event): void {
  const value = (event.target as HTMLSelectElement).value;
  change({
    query: setPrimarySort(
      config.value.query,
      value === "default" || value === "advanced" ? null : value,
      simple.value?.direction ?? "asc",
    ),
  });
}

function toggleDirection(): void {
  if (!simple.value) return;
  change({
    query: setPrimarySort(
      config.value.query,
      simple.value.field,
      simple.value.direction === "asc" ? "desc" : "asc",
    ),
  });
}

async function retryMeta(): Promise<void> {
  await Promise.all([fields.refetch(), views.refetch(), me.refetch()]);
}

function onSaveView(event: Event): void {
  event.preventDefault();
  if (
    !viewName.value.trim() ||
    saveView.isPending.value ||
    viewConflict.value ||
    shareBlocked.value ||
    invalidQuery.value
  ) {
    return;
  }
  saveView.mutate();
}

function reloadView(): void {
  const latest = views.data.value?.items.find((item) => item.id === view.value?.id) ?? null;
  saveView.reset();
  applyView(latest);
}

async function deleteCurrentView(): Promise<void> {
  if (!view.value) return;
  try {
    await removeView.mutateAsync(view.value.id);
  } catch {
    /* removeView.isError shows the message. */
  }
}

const deleteDisabled = computed(() => {
  const current = view.value;
  const userId = me.data.value?.userId;
  if (!current || removeView.isPending.value) return true;
  return current.visibility === "shared" ? !canManageViews.value : current.ownerId !== userId;
});

const showDayList = computed(
  () => !(props.type === "calendar" && config.value.dateBy && day.value === undefined),
);
const emptyCount = computed(() =>
  groupedBoard.value ? (rows.data.value?.count ?? 0) : (rows.data.value?.items.length ?? 0),
);
</script>

<template>
  <QueryError v-if="failed" :message="t('collection.error')" @retry="retryMeta" />
  <p v-else-if="!fields.data.value || !views.data.value || !me.data.value" role="status">{{
    t("collection.loading")
  }}</p>
  <section v-else class="flex min-w-0 flex-col gap-4" :data-testid="`collection-${type}`">
    <QueryError
      v-if="fieldsRefreshFailed"
      :message="loadErrorMessage(fields.error.value)"
      @retry="fields.refetch()"
    />
    <div class="collection-toolbar">
      <div class="collection-field">
        <label :for="`${baseId}-view`">{{ t("collection.savedViews") }}</label>
        <select
          :id="`${baseId}-view`"
          class="collection-select"
          :value="view?.id ?? ''"
          :disabled="removeView.isPending.value"
          @change="onSavedViewChange"
        >
          <option value="">{{ t("collection.newView") }}</option>
          <option v-for="item in views.data.value.items" :key="item.id" :value="item.id">
            {{ item.name }} ·
            {{ item.visibility === "shared" ? t("collection.shared") : t("collection.private") }}
          </option>
        </select>
      </div>
      <div v-if="type === 'board'" class="collection-field">
        <label :for="`${baseId}-group`">{{ t("collection.group") }}</label>
        <select
          :id="`${baseId}-group`"
          class="collection-select"
          :value="config.groupBy ?? ''"
          @change="onGroupBy"
        >
          <option value="">{{ t("collection.none") }}</option>
          <option value="status">{{ t("collection.status") }}</option>
          <option
            v-for="field in active.filter((item) => item.type === 'select')"
            :key="field.id"
            :value="field.id"
          >
            {{ field.name }}
          </option>
        </select>
      </div>
      <div v-if="type === 'calendar'" class="collection-field">
        <label :for="`${baseId}-date`">{{ t("collection.date") }}</label>
        <select
          :id="`${baseId}-date`"
          class="collection-select"
          :value="config.dateBy ?? ''"
          @change="onDateBy"
        >
          <option value="">{{ t("collection.none") }}</option>
          <option value="due">{{ t("collection.due") }}</option>
          <option value="start">{{ t("collection.start") }}</option>
          <option
            v-for="field in active.filter(
              (item) => item.type === 'date' || item.type === 'datetime',
            )"
            :key="field.id"
            :value="field.id"
          >
            {{ field.name }}
          </option>
        </select>
      </div>
      <div class="collection-field">
        <label :for="`${baseId}-sort`">{{ t("collection.sort") }}</label>
        <select
          :id="`${baseId}-sort`"
          class="collection-select"
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
        v-if="active.length > 0"
        size="sm"
        variant="outline"
        color="neutral"
        :aria-expanded="customOpen"
        @click="customOpen = !customOpen"
      >
        {{ t("collection.filter.custom") }}
      </UButton>
    </div>
    <CustomFilters
      v-if="customOpen && active.length > 0"
      :fields="active"
      :members="memberItems"
      :time-zone="timeZone"
      :query="config.query"
      @change="change({ query: $event })"
    />
    <form v-if="canSave" class="collection-toolbar" @submit="onSaveView">
      <div class="collection-field">
        <label :for="`${baseId}-view-name`">{{ t("collection.viewName") }}</label>
        <input
          :id="`${baseId}-view-name`"
          class="h-9 rounded-md border border-default bg-default px-3 text-sm"
          maxlength="100"
          :value="viewName"
          @input="viewName = ($event.target as HTMLInputElement).value"
        />
      </div>
      <div class="collection-field">
        <label :for="`${baseId}-visibility`">{{ t("collection.visibility") }}</label>
        <select
          :id="`${baseId}-visibility`"
          class="collection-select"
          :value="visibility"
          :disabled="!ownsView"
          @change="
            visibility =
              ($event.target as HTMLSelectElement).value === 'shared' ? 'shared' : 'private'
          "
        >
          <option value="private">{{ t("collection.private") }}</option>
          <option value="shared" :disabled="!canManageViews">{{ t("collection.shared") }}</option>
        </select>
      </div>
      <UButton
        type="submit"
        size="sm"
        :disabled="
          !viewName.trim() ||
          saveView.isPending.value ||
          removeView.isPending.value ||
          viewConflict ||
          shareBlocked ||
          invalidQuery
        "
      >
        {{ t("collection.saveView") }}
      </UButton>
      <ConfirmActionButton
        v-if="view"
        :title="t('collection.deleteView')"
        :description="t('collection.deleteView.confirm')"
        :action-label="t('project.views.delete')"
        :disabled="deleteDisabled"
        :action="deleteCurrentView"
      >
        {{ t("collection.deleteView") }}
      </ConfirmActionButton>
    </form>
    <div v-if="viewConflict" role="alert" class="collection-toolbar">
      <span class="text-sm text-error">{{ t("collection.viewConflict") }}</span>
      <UButton
        size="sm"
        variant="outline"
        color="neutral"
        :disabled="views.isFetching.value"
        @click="reloadView"
      >
        {{ t("collection.reloadView") }}
      </UButton>
    </div>
    <p
      v-if="(saveView.isError.value && !viewConflict) || removeView.isError.value"
      role="alert"
      class="text-sm text-error"
    >
      {{ t("collection.saveError") }}
    </p>
    <p v-if="moveError" role="alert" class="text-sm text-error">{{ moveError }}</p>

    <div
      v-if="type === 'calendar' && config.dateBy"
      @dragover="externalCalendarOver"
      @drop="externalCalendarDrop"
      @dragleave="onDateDragLeave"
    >
      <CollectionCalendar
        :key="`${workspaceId}:${projectId}:${collectionId}:${me.data.value.userId}`"
        ref="calendarUI"
        :month="effectiveMonth"
        :date-by="config.dateBy"
        :fields="active"
        :previews="calendarPreviews"
        :counts="dayCounts"
        :zone="timeZone"
        :week-starts-on="weekStartsOn"
        :slug="slug"
        :pending="moving"
        :refreshing="rows.isFetching.value"
        :can-edit="rows.data.value?.canEdit !== false"
        :save="saveCalendarDate"
        @month="
          month = $event;
          day = undefined;
          cursor = undefined;
        "
        @day="
          day = day === $event ? undefined : $event;
          cursor = undefined;
        "
        @range="visibleRange = $event"
        @reconnect="refresh"
      />
    </div>

    <QueryLoading v-if="rows.isPending.value" />
    <QueryError
      v-if="rows.isError.value"
      :message="loadErrorMessage(rows.error.value)"
      @retry="rows.refetch()"
    />
    <template v-if="rows.data.value">
      <p class="text-caption text-muted" role="status">{{
        t("collection.count", { count: rows.data.value.count })
      }}</p>
      <template v-if="showDayList">
        <p v-if="emptyCount === 0" class="text-sm text-muted">{{ t("collection.empty") }}</p>
        <CollectionBoard
          v-else-if="groupedBoard"
          :workspace-id="workspaceId"
          :collection-id="collectionId"
          :config="config"
          :groups="groups"
          :moving="moving"
          @move="moveToGroup"
        >
          <template #default="{ row }">
            <a :href="itemPath(slug, row.displayId)" class="font-medium hover:underline">
              <span class="text-muted">{{ row.displayId }}</span> {{ row.title }}
            </a>
            <p
              v-for="field in active"
              v-show="formatValue(field, valueOf(row, field.id))"
              :key="field.id"
              class="collection-card__meta"
            >
              {{ field.name }}: {{ formatValue(field, valueOf(row, field.id)) }}
            </p>
          </template>
        </CollectionBoard>
        <ul v-else-if="type === 'board'" class="flex flex-col gap-2">
          <li
            v-for="row in rows.data.value.items"
            :key="row.id"
            class="collection-card"
            :data-testid="`collection-card-${row.displayId}`"
          >
            <a :href="itemPath(slug, row.displayId)" class="font-medium hover:underline">
              <span class="text-muted">{{ row.displayId }}</span> {{ row.title }}
            </a>
            <p
              v-for="field in active"
              v-show="formatValue(field, valueOf(row, field.id))"
              :key="field.id"
              class="collection-card__meta"
            >
              {{ field.name }}: {{ formatValue(field, valueOf(row, field.id)) }}
            </p>
          </li>
        </ul>
        <template v-else>
          <h3 v-if="type === 'calendar'" class="text-sm font-semibold">
            {{ t("collection.dayItems", { date: day ?? t("collection.unassigned") }) }}
          </h3>
          <div class="data-table-wrap">
            <table class="data-table" data-testid="collection-table">
              <thead>
                <tr>
                  <th scope="col">{{ t("collection.resourceTitle") }}</th>
                  <th v-for="field in active" :key="field.id" scope="col">{{ field.name }}</th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="row in rows.data.value.items"
                  :key="row.id"
                  :data-testid="`collection-row-${row.displayId}`"
                >
                  <td
                    class="min-w-40"
                    :data-testid="calendarDrag ? `collection-drag-${row.displayId}` : undefined"
                    :aria-busy="calendarDrag && moving ? true : undefined"
                    :draggable="dateDraggable(row)"
                    @pointerdown="calendarDrag && calendarUI?.pointerdown($event, row)"
                    @dragstart="onDateDragStart($event, row)"
                    @dragend="onDateDragEnd"
                  >
                    <a
                      :href="itemPath(slug, row.displayId)"
                      class="font-medium hover:underline"
                      :draggable="calendarDrag ? false : undefined"
                    >
                      <span class="text-muted">{{ row.displayId }}</span> {{ row.title }}
                    </a>
                  </td>
                  <td v-for="field in active" :key="field.id" class="min-w-44">
                    <ValueEditor
                      :field="field"
                      :value="asCollectionValue(valueOf(row, field.id))"
                      :members="memberItems"
                      :time-zone="timeZone"
                      :read-only="!row.canEdit"
                      :show-label="false"
                      :label-suffix="row.displayId"
                      :save-value="(value) => setValue(row, field, value)"
                    />
                  </td>
                </tr>
              </tbody>
            </table>
          </div>
        </template>
        <div
          v-if="!groupedBoard && (cursor || rows.data.value.nextCursor)"
          class="collection-toolbar"
        >
          <UButton
            size="sm"
            variant="outline"
            color="neutral"
            :disabled="!cursor"
            @click="cursor = undefined"
          >
            {{ t("collection.previous") }}
          </UButton>
          <UButton
            size="sm"
            variant="outline"
            color="neutral"
            :disabled="!rows.data.value.nextCursor"
            @click="cursor = rows.data.value.nextCursor ?? undefined"
          >
            {{ t("collection.next") }}
          </UButton>
        </div>
      </template>
    </template>
  </section>
</template>
