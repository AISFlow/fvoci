<script setup lang="ts">
import { t } from "@fvoci/i18n";
import { computed, ref, useId } from "vue";
import type { IsoDate } from "@/lib/iso-date";
import {
  applyBarDelta,
  applyBarPointer,
  barRect,
  chartHeight,
  dateToX,
  dayColumns,
  laneCenterY,
  linkPaths,
  monthBands,
  packFlow,
  scaleWidth,
  stackRows,
  type BarBox,
  type BarDragKind,
  type GanttScale,
  type PackMode,
} from "./gantt-geometry";
import type { GanttLayout, GanttLayoutItem } from "./useGanttLayout";

export interface GanttBarChange {
  id: string;
  kind: BarDragKind;
  start: IsoDate;
  end: IsoDate;
}

const props = withDefaults(
  defineProps<{
    /** Reads items, links, calendar and the scale range; the server's pixel fields are ignored. */
    layout: GanttLayout;
    /** From the layout's canEdit and the page state; the server re-checks every change. */
    canEdit: boolean;
    /** The task being saved: drawn busy, and no bar takes a change meanwhile. */
    savingId?: string | null;
    /** A saved range to draw until the layout has it. */
    pending?: { id: string; start: IsoDate; end: IsoDate } | null;
    /** Today in the user's time zone. */
    today: IsoDate;
    pack?: PackMode;
    pxPerDay?: number;
    laneHeight?: number;
  }>(),
  { savingId: null, pending: null, pack: "rows", pxPerDay: 32, laneHeight: 48 },
);

const emit = defineEmits<{
  select: [id: string];
  change: [change: GanttBarChange];
}>();

defineSlots<{
  "rail-row"?: (props: { item: GanttLayoutItem }) => unknown;
}>();

const HANDLE_PX = 8;
const SELECT_SLOP = 4;
const CHAR_PX = 7;

const uid = useId().replace(/[^A-Za-z0-9_-]/g, "");
const markerId = `fvoci-gantt-arrow-${uid}`;

const scale = computed<GanttScale>(() => ({
  start: props.layout.scale.start,
  end: props.layout.scale.end,
  pxPerDay: props.pxPerDay,
}));
const itemsById = computed(() => new Map(props.layout.items.map((item) => [item.id, item])));
const calendar = computed(() => ({
  weekend: props.layout.calendar.weekend,
  holidays: new Set(props.layout.calendar.holidays),
}));
const packed = computed(() =>
  props.pack === "overlap"
    ? packFlow(props.layout.items, scale.value, props.layout.links)
    : stackRows(props.layout.items, scale.value),
);
const width = computed(() => scaleWidth(scale.value));
const columns = computed(() => dayColumns(scale.value, calendar.value));
const months = computed(() => monthBands(columns.value));
const laneCount = computed(() => packed.value.bars.reduce((max, bar) => Math.max(max, bar.lane + 1), 0));
const todayX = computed(() => dateToX(props.today, scale.value));
const editable = computed(() => props.canEdit);
const locked = computed(() => !props.canEdit || props.savingId !== null);

interface DisplayBar extends BarBox {
  readonly start: IsoDate;
  readonly end: IsoDate;
}

function placed(bar: BarBox, range: { start: IsoDate; end: IsoDate } | null): DisplayBar {
  const item = itemsById.value.get(bar.id)!;
  if (!range) return { ...bar, start: item.start, end: item.end };
  const rect = barRect({ start: range.start, end: range.end, milestone: bar.milestone }, scale.value);
  return rect ? { ...bar, ...rect, start: range.start, end: range.end } : { ...bar, start: item.start, end: item.end };
}

/** Committed bars, with a saved-but-not-refetched range in place. */
const committedBars = computed(() =>
  packed.value.bars.map((bar) => placed(bar, props.pending?.id === bar.id ? props.pending : null)),
);
const paths = computed(() => linkPaths(props.layout.links, committedBars.value, props.laneHeight, props.pxPerDay));
const height = computed(() => chartHeight(laneCount.value, props.laneHeight, paths.value));

// Drag state is local to the chart; only a finished change leaves it.
interface BarDrag {
  id: string;
  kind: BarDragKind;
  originStart: IsoDate;
  originEnd: IsoDate;
  originX: number;
  originClientX: number;
  start: IsoDate;
  end: IsoDate;
  moved: boolean;
}
const svg = ref<SVGSVGElement | null>(null);
let drag: BarDrag | null = null;
let skipSelect = false;
const live = ref<{ id: string; start: IsoDate; end: IsoDate } | null>(null);

const bars = computed(() =>
  committedBars.value.map((bar) => (live.value?.id === bar.id ? placed(bar, live.value) : bar)),
);

function clientToX(clientX: number): number {
  const box = svg.value?.getBoundingClientRect();
  if (!box || box.width <= 0) return 0;
  return ((clientX - box.left) / box.width) * width.value;
}

function beginDrag(event: PointerEvent, bar: DisplayBar, kind: BarDragKind): void {
  if (locked.value || event.button !== 0) return;
  event.stopPropagation();
  svg.value?.setPointerCapture(event.pointerId);
  drag = {
    id: bar.id,
    kind,
    originStart: bar.start,
    originEnd: bar.end,
    originX: clientToX(event.clientX),
    originClientX: event.clientX,
    start: bar.start,
    end: bar.end,
    moved: false,
  };
}

function onPointerMove(event: PointerEvent): void {
  if (!drag) return;
  if (Math.abs(event.clientX - drag.originClientX) >= SELECT_SLOP) drag.moved = true;
  if (!drag.moved) return;
  const next = applyBarPointer(drag, clientToX(event.clientX), scale.value);
  drag.start = next.start;
  drag.end = next.end;
  live.value = { id: drag.id, ...next };
}

function onPointerUp(event: PointerEvent): void {
  const done = drag;
  drag = null;
  if (svg.value?.hasPointerCapture(event.pointerId)) svg.value.releasePointerCapture(event.pointerId);
  live.value = null;
  if (!done) return;
  const changed = done.start !== done.originStart || done.end !== done.originEnd;
  if (done.moved || changed) {
    // The click that ends a drag does not open the task.
    skipSelect = true;
    window.setTimeout(() => {
      skipSelect = false;
    }, 0);
  }
  if (changed && event.type === "pointerup") {
    emit("change", { id: done.id, kind: done.kind, start: done.start, end: done.end });
  }
}

function onBarClick(id: string): void {
  if (skipSelect) {
    skipSelect = false;
    return;
  }
  emit("select", id);
}

/**
 * The edges a bar offers a handle for. A task with one date (inferred
 * from-start or from-due) has none on the edge its date is on: dragging that
 * edge would only move the date (reschedule-body.ts writes no date the drag
 * did not set), which a move of the bar already does.
 */
function hasHandle(bar: Pick<BarBox, "milestone" | "inferred">, edge: "start" | "end"): boolean {
  if (bar.milestone) return false;
  return edge === "start" ? bar.inferred !== "from-start" : bar.inferred !== "from-due";
}

/** Enter/Space opens the task; arrows move the bar a day, Shift+arrows move its end handle. */
function onBarKeydown(event: KeyboardEvent, bar: DisplayBar): void {
  if (event.key === "Enter" || event.key === " ") {
    event.preventDefault();
    emit("select", bar.id);
    return;
  }
  if (event.key !== "ArrowLeft" && event.key !== "ArrowRight") return;
  if (locked.value) return;
  event.preventDefault();
  const kind: BarDragKind = event.shiftKey && hasHandle(bar, "end") ? "end" : "move";
  const next = applyBarDelta(kind, bar, event.key === "ArrowLeft" ? -1 : 1, scale.value);
  if (next.start !== bar.start || next.end !== bar.end) emit("change", { id: bar.id, kind, ...next });
}

function title(id: string): string {
  return itemsById.value.get(id)?.title ?? id;
}

function barHeight(): number {
  return props.laneHeight * (props.pack === "overlap" ? 0.52 : 0.58);
}

type LabelPlacement = "inner" | "outer" | "none";

/** Where each bar's title goes: inside a wide bar, else after it unless a bar in its lane is in the way. */
const labelPlacements = computed(() => {
  const byLane = new Map<number, DisplayBar[]>();
  for (const bar of bars.value) {
    const lane = byLane.get(bar.lane);
    if (lane) lane.push(bar);
    else byLane.set(bar.lane, [bar]);
  }
  const out = new Map<string, LabelPlacement>();
  for (const bar of bars.value) {
    if (!bar.milestone && bar.width >= 56) {
      out.set(bar.id, "inner");
      continue;
    }
    const start = bar.x + bar.width + 8;
    const end = start + Math.min(title(bar.id).length * CHAR_PX, 96);
    const hits = (byLane.get(bar.lane) ?? []).some(
      (other) => other.id !== bar.id && other.x < end && other.x + other.width > start,
    );
    out.set(bar.id, hits ? (!bar.milestone && bar.width >= 32 ? "inner" : "none") : "outer");
  }
  return out;
});

function labelPlacement(bar: DisplayBar): LabelPlacement {
  return labelPlacements.value.get(bar.id) ?? "none";
}

function diamond(cx: number, cy: number, r: number): string {
  return `${cx},${cy - r} ${cx + r},${cy} ${cx},${cy + r} ${cx - r},${cy}`;
}

const rowRules = computed(() =>
  Array.from({ length: Math.max(laneCount.value, 1) }, (_, n) => (n + 1) * props.laneHeight),
);
const railRows = computed(() =>
  [...bars.value].sort((a, b) => a.lane - b.lane || a.x - b.x).map((bar) => itemsById.value.get(bar.id)!),
);
</script>

<template>
  <div
    class="fvoci-gantt fvoci-gantt--split"
    data-slot="gantt"
    :data-bar-edit="editable ? '1' : undefined"
    :data-pack="pack"
    :data-px-per-day="pxPerDay"
    :data-link-count="paths.length"
    :data-link-total="layout.linkTotal"
    :aria-busy="savingId !== null ? 'true' : undefined"
  >
    <div class="fvoci-gantt__rail">
      <div class="fvoci-gantt__rail-head" :style="{ height: `${laneHeight * 1.4}px` }">{{ t("gantt.rail") }}</div>
      <button
        v-for="item in railRows"
        :key="item.id"
        type="button"
        class="fvoci-gantt__row-label"
        :class="{ 'fvoci-gantt__bar--inferred': item.inferred !== 'none' }"
        :style="{ height: `${laneHeight}px` }"
        :data-task-id="item.id"
        @click="emit('select', item.id)"
      >
        <slot name="rail-row" :item="item">{{ item.title }}</slot>
      </button>
    </div>
    <section class="fvoci-gantt__board" :aria-label="t('view.gantt')" tabindex="0">
      <div class="fvoci-gantt__header" :style="{ width: `${width}px`, height: `${laneHeight * 1.4}px` }">
        <span
          v-for="band in months"
          :key="band.key"
          class="fvoci-gantt__month"
          :style="{ left: `${band.x}px`, width: `${band.width}px` }"
          >{{ band.label }}</span
        >
        <span
          v-for="column in columns"
          :key="column.date"
          class="fvoci-gantt__tick"
          :class="{
            'fvoci-gantt__col--offduty': column.offDuty,
            'fvoci-gantt__day--today': column.date === today,
          }"
          :style="{ left: `${column.x}px`, width: `${column.width}px` }"
          :data-date="column.date"
          :data-off-duty="column.offDuty ? '1' : undefined"
        >
          <span :class="column.date === today ? 'fvoci-gantt__tick-today' : 'fvoci-gantt__tick-label'">{{
            column.label
          }}</span>
        </span>
      </div>
      <svg
        ref="svg"
        class="fvoci-gantt__canvas"
        :width="width"
        :height="height"
        overflow="visible"
        role="group"
        :aria-label="t('gantt.chart.aria', { count: bars.length, start: layout.scale.start, end: layout.scale.end })"
        @pointermove="onPointerMove"
        @pointerup="onPointerUp"
        @pointercancel="onPointerUp"
      >
        <title>{{ t("gantt.chart.title", { start: layout.scale.start, end: layout.scale.end }) }}</title>
        <defs>
          <marker
            :id="markerId"
            markerWidth="8"
            markerHeight="8"
            refX="7"
            refY="4"
            orient="auto"
            markerUnits="userSpaceOnUse"
          >
            <path d="M0 0 L8 4 L0 8 z" class="fvoci-gantt__link-head" />
          </marker>
        </defs>
        <template v-for="column in columns" :key="`off-${column.date}`">
          <rect
            v-if="column.offDuty"
            class="fvoci-gantt__col--offduty"
            :x="column.x"
            :y="0"
            :width="column.width"
            :height="height"
            pointer-events="none"
          />
        </template>
        <line
          v-for="y in rowRules"
          :key="y"
          class="fvoci-gantt__row-rule"
          :x1="0"
          :x2="width"
          :y1="y"
          :y2="y"
          pointer-events="none"
        />
        <template v-if="todayX !== null && todayX >= 0 && todayX <= width">
          <rect
            class="fvoci-gantt__today-band"
            :x="todayX"
            :y="0"
            :width="Math.max(pxPerDay, 2)"
            :height="height"
            pointer-events="none"
          />
          <line
            class="fvoci-gantt__today-line"
            :x1="todayX"
            :x2="todayX"
            :y1="0"
            :y2="height"
            pointer-events="none"
          />
        </template>
        <g
          v-for="bar in bars"
          :key="bar.id"
          class="fvoci-gantt__bar"
          :class="{
            'fvoci-gantt__bar--inferred': bar.inferred !== 'none',
            'fvoci-gantt__bar--live': live?.id === bar.id,
            'fvoci-gantt__bar--saving': savingId === bar.id,
          }"
          role="button"
          tabindex="0"
          :aria-label="t('gantt.bar.aria', { title: title(bar.id), start: bar.start, end: bar.end })"
          :aria-busy="savingId === bar.id ? 'true' : undefined"
          :aria-keyshortcuts="
            !editable
              ? 'Enter'
              : hasHandle(bar, 'end')
                ? 'Enter ArrowLeft ArrowRight Shift+ArrowLeft Shift+ArrowRight'
                : 'Enter ArrowLeft ArrowRight'
          "
          :data-task-id="bar.id"
          :data-start="bar.start"
          :data-end="bar.end"
          :data-saving="savingId === bar.id ? '1' : undefined"
          @click="onBarClick(bar.id)"
          @keydown="onBarKeydown($event, bar)"
        >
          <title>{{ `${title(bar.id)} (${bar.milestone ? t("gantt.milestone") : t("gantt.kind.bar")})` }}</title>
          <rect
            v-if="pack === 'rows'"
            class="fvoci-gantt__hit"
            :x="0"
            :y="bar.lane * laneHeight"
            :width="width"
            :height="laneHeight"
            fill="transparent"
          />
          <clipPath v-if="labelPlacement(bar) === 'inner'" :id="`${markerId}-clip-${bar.id}`">
            <rect
              :x="bar.x"
              :y="laneCenterY(bar.lane, laneHeight) - barHeight() / 2"
              :width="Math.max(bar.width, 2)"
              :height="barHeight()"
              :rx="barHeight() / 2"
            />
          </clipPath>
          <polygon
            v-if="bar.milestone"
            class="fvoci-gantt__milestone"
            :points="diamond(bar.x + bar.width / 2, laneCenterY(bar.lane, laneHeight), bar.width / 2)"
            @pointerdown="beginDrag($event, bar, 'move')"
          />
          <rect
            v-else
            class="fvoci-gantt__bar-rect"
            :x="bar.x"
            :y="laneCenterY(bar.lane, laneHeight) - barHeight() / 2"
            :width="Math.max(bar.width, 2)"
            :height="barHeight()"
            :rx="barHeight() / 2"
            @pointerdown="beginDrag($event, bar, 'move')"
          />
          <template v-if="editable">
            <rect
              v-if="hasHandle(bar, 'start')"
              class="fvoci-gantt__handle"
              data-handle="start"
              :x="bar.x"
              :y="laneCenterY(bar.lane, laneHeight) - barHeight() / 2"
              :width="HANDLE_PX"
              :height="barHeight()"
              @pointerdown="beginDrag($event, bar, 'start')"
            />
            <rect
              v-if="hasHandle(bar, 'end')"
              class="fvoci-gantt__handle"
              data-handle="end"
              :x="bar.x + Math.max(bar.width, 2) - HANDLE_PX"
              :y="laneCenterY(bar.lane, laneHeight) - barHeight() / 2"
              :width="HANDLE_PX"
              :height="barHeight()"
              @pointerdown="beginDrag($event, bar, 'end')"
            />
          </template>
        </g>
        <polyline
          v-for="path in paths"
          :key="`${path.blockerId}->${path.blockedId}`"
          class="fvoci-gantt__link"
          :points="path.points.join(' ')"
          fill="none"
          :marker-end="`url(#${markerId})`"
          pointer-events="none"
          :data-blocker-id="path.blockerId"
          :data-blocked-id="path.blockedId"
        />
        <template v-for="bar in bars" :key="`label-${bar.id}`">
          <text
            v-if="labelPlacement(bar) !== 'none'"
            class="fvoci-gantt__bar-label"
            :class="{ 'fvoci-gantt__bar-label--outside': labelPlacement(bar) === 'outer' }"
            :clip-path="labelPlacement(bar) === 'inner' ? `url(#${markerId}-clip-${bar.id})` : undefined"
            :x="labelPlacement(bar) === 'inner' ? bar.x + 8 : bar.x + bar.width + 8"
            :y="laneCenterY(bar.lane, laneHeight)"
            dy="0.35em"
            pointer-events="none"
          >
            {{ title(bar.id) }}
          </text>
        </template>
      </svg>
    </section>
  </div>
</template>
