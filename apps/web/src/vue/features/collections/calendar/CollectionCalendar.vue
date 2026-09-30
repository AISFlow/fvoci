<script setup lang="ts">
// UI adapted from nuxt-ui-templates/calendar @11809148: page header, MonthWeek,
// WeekView point placement, EventChip/Popover, Mini, useCalendar/useEventMove.
// MIT Copyright (c) 2025 Nuxt UI Templates; full notice maintained centrally.
import { computed, onBeforeUnmount, onMounted, ref, watch } from "vue";
import UButton from "@nuxt/ui/components/Button.vue";
import UTabs from "@nuxt/ui/components/Tabs.vue";
import UPopover from "@nuxt/ui/components/Popover.vue";
import { t } from "@fvoci/i18n";
import { isIsoDate } from "@/features/tasks/task-edit-payload";
import { monthGrid, shiftMonth, todayInTimeZone } from "@/lib/collection-values";
import { weekdayNames } from "@/features/collections/collection-view";
import { CALENDAR_DRAG_TYPE, dateMovable } from "@/features/collections/calendar-model";
import type { CollectionField, CollectionQueryPreview } from "@/lib/queries/collections";
import {
  addDays,
  dayMove,
  editorWrite,
  eventFor,
  resizable,
  resizeWrite,
  weekDays,
  type CalendarEvent,
  type CalendarView,
  type CalendarWrite,
} from "./calendar-adapter";
import CalendarMini from "./CalendarMini.vue";
import CalendarEventEditor from "./CalendarEventEditor.vue";
import "./calendar.css";
const props = defineProps<{
  month: string;
  dateBy: string;
  fields: readonly CollectionField[];
  previews: readonly CollectionQueryPreview[];
  counts: Map<string | null, number>;
  zone: string;
  weekStartsOn: number;
  slug: string;
  pending: boolean;
  refreshing: boolean;
  canEdit: boolean;
  save: (row: CollectionQueryPreview, write: CalendarWrite) => Promise<void>;
}>();
const emit = defineEmits<{
  month: [month: string];
  day: [day: string | null];
  range: [range: { from: string; to: string }];
  reconnect: [];
}>();
const view = ref<CalendarView>("month");
const anchor = ref(props.month + "-01");
const sidebar = ref(false);
const online = ref(typeof navigator === "undefined" || navigator.onLine);
const editor = ref<CalendarEvent | null>(null);
const editorOpen = ref(false);
const editorBasis = ref(props.dateBy);
const editorResizeEdge = ref<"start" | "end" | null>(null);
const editorEvent = computed(() =>
  editor.value
    ? {
        ...editor.value,
        canEdit:
          editor.value.canEdit &&
          props.canEdit &&
          (events.value.find((event) => event.id === editor.value?.id)?.canEdit ?? true),
      }
    : null,
);
const dragged = ref<CalendarEvent | null>(null);
const resizeEdge = ref<"start" | "end" | null>(null);
const over = ref<string | null | undefined>();
const suppressed = ref(false);
const nativeDragging = ref(false);
const root = ref<HTMLElement>();
const popoverReference = ref<{ getBoundingClientRect: () => DOMRect }>();
const today = computed(() => todayInTimeZone(props.zone));
const weeks = computed(() => monthGrid(props.month, props.weekStartsOn));
const days = computed(() =>
  view.value === "day" ? [anchor.value] : weekDays(anchor.value, props.weekStartsOn),
);
const events = computed(() =>
  props.previews.map((row) => eventFor(row, props.dateBy, props.fields, props.zone)),
);
const title = computed(() =>
  t("cal.yearMonth", { year: props.month.slice(0, 4), month: Number(props.month.slice(5)) }),
);
const tabs = [
  { label: "Day", value: "day" },
  { label: "Week", value: "week" },
  { label: "Month", value: "month" },
];
const hours = Array.from({ length: 24 }, (_, i) => i);
watch(
  () => props.month,
  (month) => {
    if (!anchor.value.startsWith(month)) anchor.value = `${month}-01`;
  },
);
watch(
  [view, anchor, weeks],
  () => {
    const range = view.value === "month" ? weeks.value.flat().map((cell) => cell.date) : days.value;
    emit("range", { from: range[0]!, to: addDays(range.at(-1)!, 1) });
  },
  { immediate: true },
);
function select(day: string) {
  if (!isIsoDate(day)) return;
  anchor.value = day;
  emit("month", day.slice(0, 7));
  sidebar.value = false;
}
function shift(delta: number) {
  select(
    view.value === "month"
      ? shiftMonth(props.month, delta) + "-01"
      : addDays(anchor.value, delta * (view.value === "week" ? 7 : 1)),
  );
}
function open(event: CalendarEvent, trigger?: Event, edge?: "start" | "end") {
  if (suppressed.value) return;
  const rect = (trigger?.currentTarget as HTMLElement | undefined)?.getBoundingClientRect();
  if (rect) popoverReference.value = { getBoundingClientRect: () => rect };
  editorResizeEdge.value = edge ?? null;
  editorBasis.value = edge === "start" ? "start" : edge === "end" ? "due" : props.dateBy;
  editor.value = {
    ...event,
    local: edge === "start" ? event.startDate! : edge === "end" ? event.dueDate! : event.local,
    timed: edge ? false : event.timed,
  };
  editorOpen.value = true;
}
function list(day: string) {
  select(day);
  emit("day", day);
}
function eventsAt(day: string, hour?: number) {
  return events.value.filter(
    (event) =>
      event.date === day &&
      (hour === undefined ||
        (hour === -1 ? !event.timed : event.timed && Number(event.local.slice(11, 13)) === hour)),
  );
}
function movable(event: CalendarEvent) {
  return (
    online.value &&
    props.canEdit &&
    !props.pending &&
    dateMovable(props.dateBy, event, props.fields)
  );
}
function start(event: DragEvent, row: CalendarEvent, edge: "start" | "end" | null = null) {
  if (!movable(row)) {
    event.preventDefault();
    return;
  }
  nativeDragging.value = true;
  gesture = null;
  dragged.value = row;
  resizeEdge.value = edge;
  event.dataTransfer?.setData(CALENDAR_DRAG_TYPE, row.id);
  if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
}
function request(row: CalendarEvent, day: string | null, hour?: number): CalendarWrite | null {
  if (resizeEdge.value) return day ? resizeWrite(row, props.dateBy, resizeEdge.value, day) : null;
  if (day && hour !== undefined && hour >= 0 && row.timed) {
    // A timed point stays an instant: no dueDate conversion on the time axis.
    return editorWrite(
      props.dateBy,
      row,
      `${day}T${String(hour).padStart(2, "0")}:${row.local.slice(14, 16)}`,
      true,
      props.fields,
      props.zone,
    );
  }
  return dayMove(props.dateBy, row, day, props.fields, props.zone);
}
function dragover(event: DragEvent, day: string | null, hour?: number) {
  // Allow external day-list native drags; their parent preserves snapshot/guards.
  if (!online.value || props.pending) return;
  if (dragged.value && !request(dragged.value, day, hour)) return;
  event.preventDefault();
  over.value = day;
}
async function drop(event: DragEvent, day: string | null, hour?: number) {
  const row = dragged.value;
  if (!row) return;
  const write = request(row, day, hour);
  if (event.dataTransfer?.getData(CALENDAR_DRAG_TYPE) !== row.id) return;
  event.preventDefault();
  event.stopPropagation();
  reset();
  if (write && online.value && !props.pending) {
    try {
      await props.save(row, write);
    } catch {
      /* parent shows error and rolls back */
    }
  }
}
function reset() {
  nativeDragging.value = false;
  dragged.value = null;
  resizeEdge.value = null;
  over.value = undefined;
}
let gesture: {
  row: CalendarEvent;
  edge: "start" | "end" | null;
  x: number;
  y: number;
  moved: boolean;
} | null = null;
function pointerdown(event: PointerEvent, row: CalendarEvent, edge: "start" | "end" | null = null) {
  if (event.button !== 0 || event.pointerType === "touch" || !movable(row)) return;
  gesture = { row, edge, x: event.clientX, y: event.clientY, moved: false };
}
function pointermove(event: PointerEvent) {
  if (
    !gesture ||
    (Math.hypot(event.clientX - gesture.x, event.clientY - gesture.y) < 5 && !gesture.moved)
  )
    return;
  gesture.moved = true;
  suppressed.value = true;
  dragged.value = gesture.row;
  resizeEdge.value = gesture.edge;
  const cell = document
    .elementFromPoint(event.clientX, event.clientY)
    ?.closest<HTMLElement>("[data-calendar-target]");
  if (cell && root.value?.contains(cell)) over.value = cell.dataset.calendarTarget || null;
}
function pointerup(event: PointerEvent) {
  // Native HTML drag dispatches pointercancel at dragstart; it owns drop/end.
  if (nativeDragging.value) return;
  const row = gesture?.row;
  const moved = gesture?.moved;
  const cell = document
    .elementFromPoint(event.clientX, event.clientY)
    ?.closest<HTMLElement>("[data-calendar-target]");
  if (row && moved && cell && root.value?.contains(cell) && event.type !== "pointercancel") {
    const write = request(
      row,
      cell.dataset.calendarTarget || null,
      cell.dataset.hour ? Number(cell.dataset.hour) : undefined,
    );
    if (write && online.value && !props.pending) void props.save(row, write).catch(() => {});
  }
  gesture = null;
  reset();
  setTimeout(() => {
    suppressed.value = false;
  }, 0);
}
function cancel() {
  gesture = null;
  reset();
  suppressed.value = false;
}
function keydown(event: KeyboardEvent) {
  if (event.key === "Escape") {
    cancel();
    return;
  }
  if (
    editorOpen.value ||
    event.isComposing ||
    event.ctrlKey ||
    event.metaKey ||
    event.altKey ||
    (event.target as HTMLElement).closest("input,select,textarea,[contenteditable=true]")
  )
    return;
  if (!root.value?.contains(event.target as Node)) return;
  const switches: Record<string, CalendarView> = { d: "day", w: "week", m: "month" };
  if (switches[event.key]) {
    view.value = switches[event.key]!;
    event.preventDefault();
  } else if (event.key === "t") {
    select(today.value);
    event.preventDefault();
  } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
    shift(event.key === "ArrowLeft" ? -1 : 1);
    event.preventDefault();
  }
}
function offline() {
  online.value = false;
  cancel();
}
function reconnect() {
  online.value = true;
  emit("reconnect");
}
function reloadEditor() {
  const latest = events.value.find((event) => event.id === editor.value?.id);
  if (latest) editor.value = { ...latest };
}
defineExpose({
  pointerdown: (event: PointerEvent, row: CollectionQueryPreview) =>
    pointerdown(event, eventFor(row, props.dateBy, props.fields, props.zone)),
  nativeStart: (row: CollectionQueryPreview) => {
    nativeDragging.value = true;
    gesture = null;
    dragged.value = eventFor(row, props.dateBy, props.fields, props.zone);
    resizeEdge.value = null;
  },
  cancel,
});
onMounted(() => {
  window.addEventListener("online", reconnect);
  window.addEventListener("offline", offline);
  window.addEventListener("keydown", keydown);
  window.addEventListener("pointermove", pointermove);
  window.addEventListener("pointerup", pointerup);
  window.addEventListener("pointercancel", pointerup);
  window.addEventListener("blur", cancel);
});
onBeforeUnmount(() => {
  window.removeEventListener("online", reconnect);
  window.removeEventListener("offline", offline);
  window.removeEventListener("keydown", keydown);
  window.removeEventListener("pointermove", pointermove);
  window.removeEventListener("pointerup", pointerup);
  window.removeEventListener("pointercancel", pointerup);
  window.removeEventListener("blur", cancel);
});
</script>

<template>
  <div
    ref="root"
    class="template-calendar rounded-xl border border-default"
    tabindex="-1"
    @dragend="cancel"
  >
    <header class="flex flex-wrap items-center gap-2 border-b border-default p-3">
      <UButton
        class="lg:hidden"
        icon="i-lucide-panel-left"
        color="neutral"
        variant="ghost"
        aria-label="Calendar sidebar"
        :aria-expanded="sidebar"
        @click="sidebar = !sidebar"
      />
      <h2 class="min-w-0 flex-1 truncate text-xl font-semibold">{{ title }}</h2>
      <UTabs
        :items="tabs"
        :content="false"
        :model-value="view"
        color="neutral"
        size="sm"
        class="w-44"
        @update:model-value="view = $event as CalendarView"
      />
      <div class="flex items-center gap-1">
        <UButton
          icon="i-lucide-chevron-left"
          color="neutral"
          variant="ghost"
          :aria-label="t('cal.prevMonth')"
          @click="shift(-1)"
        />
        <UButton color="neutral" variant="outline" @click="select(today)">{{
          t("cal.today")
        }}</UButton>
        <UButton
          icon="i-lucide-chevron-right"
          color="neutral"
          variant="ghost"
          :aria-label="t('cal.nextMonth')"
          @click="shift(1)"
        />
        <input
          type="month"
          class="collection-select w-36"
          aria-label="Calendar month"
          :value="month"
          @change="select(($event.target as HTMLInputElement).value + '-01')"
        />
      </div>
      <!-- Keep drop targets stationary as save/refetch status changes mid-drag. -->
      <div class="w-full min-h-5">
        <p v-if="!online || refreshing || pending" role="status" class="text-sm text-muted">{{
          !online
            ? "Offline · unsaved drafts stay in this open editor; reconnect to save"
            : pending
              ? t("gantt.bar.saving")
              : "Refreshing from server"
        }}</p>
      </div>
    </header>
    <div class="flex min-w-0">
      <aside
        class="calendar-sidebar w-52 shrink-0 flex-col gap-3 border-e border-default p-3"
        :class="sidebar ? 'flex' : 'hidden lg:flex'"
      >
        <CalendarMini
          :month="month"
          :selected="anchor"
          :today="today"
          :week-starts-on="weekStartsOn"
          @select="select"
        />
        <p class="text-xs text-muted break-all">{{ zone }}</p>
        <p class="text-xs text-muted">d / w / m · ← / → · t</p>
        <button
          type="button"
          class="collection-calendar__none"
          :data-drop-over="over === null ? 'true' : undefined"
          data-calendar-target=""
          @click="emit('day', null)"
          @dragover="dragover($event, null)"
          @drop="drop($event, null)"
          >{{ t("collection.unassigned") }} · {{ counts.get(null) ?? 0 }}</button
        >
      </aside>
      <div class="min-w-0 flex-1 overflow-auto">
        <table
          v-if="view === 'month'"
          class="collection-calendar calendar-month w-full"
          :aria-label="title"
          data-testid="collection-calendar"
        >
          <thead
            ><tr
              ><th v-for="name in weekdayNames(weekStartsOn)" :key="name" scope="col">{{
                name
              }}</th></tr
            ></thead
          >
          <tbody
            ><tr v-for="week in weeks" :key="week[0]!.date"
              ><td
                v-for="cell in week"
                :key="cell.date"
                :data-date="cell.date"
                :data-calendar-target="cell.date"
                :data-outside="cell.inMonth ? undefined : true"
                :data-drop-over="over === cell.date ? 'true' : undefined"
                @dragover="dragover($event, cell.date)"
                @drop="drop($event, cell.date)"
              >
                <button
                  type="button"
                  class="collection-calendar__day"
                  :aria-label="`${cell.date} · ${t('collection.count', { count: counts.get(cell.date) ?? 0 })}`"
                  :data-today="cell.date === today ? true : undefined"
                  @click="list(cell.date)"
                  >{{ Number(cell.date.slice(8)) }}</button
                >
                <div
                  v-for="event in eventsAt(cell.date)"
                  :key="event.id"
                  class="calendar-event-wrap"
                >
                  <button
                    type="button"
                    data-event
                    class="calendar-chip"
                    :class="event.timed ? 'calendar-chip--timed' : ''"
                    :data-testid="`collection-preview-${event.displayId}`"
                    :draggable="movable(event)"
                    :aria-busy="pending || undefined"
                    @dragstart="start($event, event)"
                    @pointerdown="pointerdown($event, event)"
                    @click.stop="open(event, $event)"
                    ><span class="truncate">{{ event.title }}</span
                    ><time v-if="event.timed" class="text-muted">{{
                      event.local.slice(11)
                    }}</time></button
                  >
                  <span v-if="resizable(event, dateBy)" class="calendar-range text-muted"
                    >{{ event.startDate }} ~ {{ event.dueDate
                    }}<button
                      v-for="edge in ['start', 'end'] as const"
                      :key="edge"
                      type="button"
                      :aria-label="`Resize ${edge} · ${event.title}`"
                      :disabled="!movable(event)"
                      :draggable="movable(event)"
                      @dragstart.stop="start($event, event, edge)"
                      @pointerdown.stop="pointerdown($event, event, edge)"
                      @click="open(event, $event, edge)"
                      >{{ edge === "start" ? "↤" : "↦" }}</button
                    ></span
                  >
                </div>
                <button
                  v-if="(counts.get(cell.date) ?? 0) > eventsAt(cell.date).length"
                  type="button"
                  class="text-xs text-muted"
                  @click="list(cell.date)"
                  >{{
                    t("collection.more", {
                      count: (counts.get(cell.date) ?? 0) - eventsAt(cell.date).length,
                    })
                  }}</button
                >
              </td></tr
            ></tbody
          >
        </table>
        <div
          v-else
          class="calendar-week"
          data-testid="calendar-time-grid"
          :style="{ gridTemplateColumns: `3rem repeat(${days.length}, minmax(100px, 1fr))` }"
        >
          <div />
          <button
            v-for="date in days"
            :key="date"
            class="sticky top-0 bg-default p-2 text-sm font-medium"
            @click="list(date)"
            >{{ date }}</button
          >
          <span class="text-xs text-muted p-1">Date</span>
          <div
            v-for="date in days"
            :key="`date-${date}`"
            class="calendar-slot"
            :data-calendar-target="date"
            @dragover="dragover($event, date)"
            @drop="drop($event, date)"
            ><button
              v-for="event in eventsAt(date, -1)"
              :key="event.id"
              type="button"
              class="calendar-chip"
              :data-testid="`collection-preview-${event.displayId}`"
              :draggable="movable(event)"
              @dragstart="start($event, event)"
              @pointerdown="pointerdown($event, event)"
              @click="open(event, $event)"
              >{{ event.title }}</button
            ></div
          >
          <template v-for="hour in hours" :key="hour"
            ><span class="p-1 text-xs text-muted tabular-nums"
              >{{ String(hour).padStart(2, "0") }}:00</span
            ><div
              v-for="date in days"
              :key="`${date}-${hour}`"
              class="calendar-slot"
              :data-calendar-target="date"
              :data-hour="hour"
              :data-drop-over="over === date ? true : undefined"
              @dragover="dragover($event, date, hour)"
              @drop="drop($event, date, hour)"
              ><button
                v-for="event in eventsAt(date, hour)"
                :key="event.id"
                type="button"
                class="calendar-chip calendar-chip--timed"
                :data-testid="`collection-preview-${event.displayId}`"
                :draggable="movable(event)"
                @dragstart="start($event, event)"
                @pointerdown="pointerdown($event, event)"
                @click="open(event, $event)"
                ><span>• {{ event.local.slice(11) }} {{ event.title }}</span></button
              ></div
            ></template
          >
        </div>
      </div>
    </div>
    <!-- Stable host: refetch and moving a chip cannot discard the editor's draft. -->
    <UPopover
      :reference="popoverReference"
      :open="editorOpen"
      :ui="{ content: 'p-0' }"
      @update:open="editorOpen = $event"
    >
      <button v-if="editor" type="button" class="sr-only" aria-label="Selected calendar event">{{
        editor.title
      }}</button>
      <template #content
        ><CalendarEventEditor
          v-if="editorEvent"
          :key="`${editorEvent.id}-${editorBasis}-${editorResizeEdge}`"
          :event="editorEvent"
          :date-by="editorBasis"
          :resize-edge="editorResizeEdge"
          :fields="fields"
          :zone="zone"
          :slug="slug"
          :pending="pending"
          :online="online"
          :save="save"
          @close="editorOpen = false"
          @reload="reloadEditor"
      /></template>
    </UPopover>
  </div>
</template>
