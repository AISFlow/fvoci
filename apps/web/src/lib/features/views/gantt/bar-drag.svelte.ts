/**
 * Dragging the bars of a Gantt chart: the bar across to other days, or one
 * of its ends (its grips) to make it longer or shorter.
 *
 * A mouse takes a bar at the press and a finger after resting on it
 * (`pointerDrag`, `immediate`): a finger that moves at once scrolls the
 * chart, and a press let go where it was is a click, which opens the card.
 * Escape does not cancel; the bar put back where it was is the way to.
 *
 * While a bar is dragged it is drawn on the days it would go to
 * (`preview`); the board changes only when it is let go, in one step that
 * one undo takes back. Near the left or
 * right edge of the days in sight the chart scrolls, and the axis grows
 * (`reach`) where the bar or the pointer comes near its end; the chart keeps
 * the days in sight in place when the axis grows at its start (DaysHold).
 */
import { pointerDrag, type PointerDragSession } from '$lib/drag/pointer-drag.ts';
import { DRAG_SETTINGS } from '$lib/drag/settings.ts';
import { draggedDays, type BarGrip } from '$lib/logic/calendar.ts';
import { addDays, calendarDate, daysBetween, type CalendarDate } from '$lib/logic/dates.ts';
import type { Days } from '$lib/logic/dependencies.ts';
import { edgeScrollFactor, type Point } from '$lib/logic/drop-target.ts';
import type { Axis } from '$lib/logic/gantt.ts';

export type { BarGrip };

export interface BarDragSource {
  axis: () => Axis;
  /** The days of a card's bar; none where it has none. */
  days: (cardId: string) => Days | undefined;
  /** Bars may be dragged: by someone who may edit, where the chart is placed by a date to set. */
  enabled: () => boolean;
  /** The box that scrolls; undefined while it is not drawn. */
  scroller: () => HTMLElement | undefined;
  /** How wide the column of titles is, in pixels. */
  titles: () => number;
  /** How wide the days in sight are, in pixels. */
  room: () => number;
  /** The size of 1rem in pixels. */
  rem: () => number;
  /** A bar was let go on other days than its own. */
  ondrop: (cardId: string, grip: BarGrip, days: Days) => void;
}

/** The bar being dragged, and the days it would go to. */
export interface BarPreview {
  id: string;
  grip: BarGrip;
  days: Days;
}

/** Days are counted from here, with their fraction: any day does. */
const FROM = calendarDate(2000, 1, 1);
/**
 * How far the axis grows at a time, beyond what the drag needs: twice the
 * days in sight, and four weeks at least. In pixels and not in days, so
 * that a chart scrolled at the edge at a far zoom grows a few times a
 * second at most, each time drawing every bar again.
 */
const GROWTH_SIGHTS = 2;
const GROWTH_DAYS = 28;
const FIRST_DAY = calendarDate(1, 1, 1);
const LAST_DAY = calendarDate(9999, 12, 31);

const earlier = (a: CalendarDate, b: CalendarDate): CalendarDate => (a < b ? a : b);
const later = (a: CalendarDate, b: CalendarDate): CalendarDate => (a > b ? a : b);

interface Drag {
  id: string;
  grip: BarGrip;
  days: Days;
  /** Where it was pressed. */
  press: Point;
  /**
   * The day under the press, with its fraction, counted from FROM: read when
   * the drag shows. A press may first give a bar the focus, which scrolls
   * a bar that begins under the titles out from under them.
   */
  origin: number;
  last: Point;
  frame: number | undefined;
  /** When the last frame was, and the part of a pixel not yet scrolled. */
  time: number;
  owed: number;
}

export class BarDrag {
  readonly #source: BarDragSource;
  /** The bar being dragged and where it would go; none while nothing is dragged far enough to show. */
  preview = $state.raw<BarPreview | undefined>(undefined);
  /**
   * Where the days of the dragged bar are said: at the pointer (`x`, in rem
   * from the start of the axis), which lies as far along the words (`part`,
   * 0 to 1) as it is across the days in sight. The pointer is always in
   * sight, and so are the words, as long as they are no wider than the days
   * in sight; an end of a long bar may not be.
   */
  label = $state.raw<{ x: number; part: number } | undefined>(undefined);
  /** Days the axis shows beyond the cards' while a bar is dragged near or past its ends. */
  reach = $state.raw<Days | undefined>(undefined);
  #drag: Drag | undefined;

  constructor(source: BarDragSource) {
    this.#source = source;
  }

  /** The box the bars are in. For `{@attach}`. */
  readonly attach = (root: HTMLElement): (() => void) => {
    const handle = pointerDrag(root, this.#press, { immediate: true, escapeCancels: false });
    return () => {
      handle.destroy();
    };
  };

  /** The day under a point, with its fraction, counted from FROM. */
  #dayAt(point: Point): number {
    const scroller = this.#source.scroller();
    const axis = this.#source.axis();
    if (!scroller) return 0;
    const left =
      scroller.getBoundingClientRect().left +
      scroller.clientLeft +
      this.#source.titles() -
      scroller.scrollLeft;
    return daysBetween(FROM, axis.first) + (point.x - left) / (axis.dayWidth * this.#source.rem());
  }

  readonly #press = (event: PointerEvent): PointerDragSession | undefined => {
    if (!this.#source.enabled()) return undefined;
    const target = event.target instanceof Element ? event.target : null;
    // On the bar or a grip: not on the title, under which a grip may lie.
    const row = target?.closest('[data-bar], [data-grip]')?.closest<HTMLElement>('[data-row]');
    const id = row?.dataset.row;
    if (!row || id === undefined) return undefined;
    // Where it was pressed, not what the browser gave the press to: for a
    // finger it moves a press to the nearest thing that can be pressed, the
    // bar beside a grip. On the bar itself, the bar is taken.
    const within = (element: Element | null | undefined): boolean => {
      const box = element?.getBoundingClientRect();
      return box !== undefined && event.clientX >= box.left && event.clientX <= box.right;
    };
    const onBar = within(row.querySelector('[data-bar]'));
    const grip = onBar
      ? undefined
      : [...row.querySelectorAll<HTMLElement>('[data-grip]')].find((each) => within(each));
    const days = this.#source.days(id);
    if (!days) return undefined;
    const which: BarGrip = grip ? (grip.dataset.grip === 'start' ? 'start' : 'end') : 'move';
    const shownAtOnce = event.pointerType !== 'mouse';
    return {
      start: (point) => {
        this.#drag = {
          id,
          grip: which,
          days,
          press: point,
          origin: 0,
          last: point,
          frame: undefined,
          time: 0,
          owed: 0,
        };
        // A finger waited for the bar: it is shown taken. A mouse may still be clicking.
        if (shownAtOnce) this.#show();
      },
      move: (point) => {
        if (!this.#drag) return;
        this.#drag.last = point;
        if (this.#drag.frame === undefined) this.#show();
        this.#update();
      },
      drop: (point) => {
        const drag = this.#drag;
        if (!drag) return;
        drag.last = point;
        this.#update();
        const preview = this.preview;
        this.#end();
        if (!preview) return;
        if (preview.days.first !== drag.days.first || preview.days.last !== drag.days.last) {
          this.#source.ondrop(drag.id, drag.grip, preview.days);
        }
      },
      cancel: () => {
        this.#end();
      },
    };
  };

  /** The bar is shown taken, and the frames for scrolling begin. */
  #show(): void {
    const drag = this.#drag;
    if (!drag) return;
    drag.origin = this.#dayAt(drag.press);
    this.preview = { id: drag.id, grip: drag.grip, days: drag.days };
    drag.time = performance.now();
    drag.frame = requestAnimationFrame(this.#frame);
  }

  /** Where the pointer now is, in days: the copy goes there, and the axis grows where it must. */
  #update(): void {
    const drag = this.#drag;
    if (!drag) return;
    const at = this.#dayAt(drag.last);
    const days = draggedDays(drag.days, drag.grip, Math.round(at - drag.origin));
    const shown = this.preview;
    if (shown?.days.first !== days.first || shown.days.last !== days.last) {
      this.preview = { id: drag.id, grip: drag.grip, days };
    }
    this.#grow(days, Math.floor(at));
    this.#place(at, drag.last);
  }

  /**
   * The axis grows to take the dragged days and those around the pointer, a
   * few weeks at a time: it is worked out again (and every bar with it) only
   * when the drag comes near an end.
   */
  #grow(days: Days, pointer: number): void {
    const axis = this.#source.axis();
    const day = axis.dayWidth * this.#source.rem();
    const edge = Math.ceil(DRAG_SETTINGS.scrollEdge / day) + 1;
    const near = addDays(FROM, pointer);
    const first = later(FIRST_DAY, earlier(days.first, addDays(near, -edge)));
    const last = earlier(LAST_DAY, later(days.last, addDays(near, edge)));
    if (first >= axis.first && last <= axis.last) return;
    const growth = Math.max(GROWTH_DAYS, Math.ceil((GROWTH_SIGHTS * this.#source.room()) / day));
    const reach = this.reach;
    const next = {
      first: earlier(
        reach?.first ?? first,
        first < axis.first ? later(FIRST_DAY, addDays(first, -growth)) : first,
      ),
      last: later(
        reach?.last ?? last,
        last > axis.last ? earlier(LAST_DAY, addDays(last, growth)) : last,
      ),
    };
    // At the first or the last day there is, or where the axis is as long as
    // it may be (MAX_AXIS_DAYS), it cannot grow: nothing changes.
    if (next.first === reach?.first && next.last === reach.last) return;
    this.reach = next;
  }

  /** One frame of a drag: the days scroll while the pointer is near their left or right edge. */
  readonly #frame = (now: number): void => {
    const drag = this.#drag;
    const scroller = this.#source.scroller();
    if (!drag || !scroller) return;
    const elapsed = Math.min(now - drag.time, DRAG_SETTINGS.longestFrame);
    drag.time = now;
    const rect = scroller.getBoundingClientRect();
    const left = rect.left + this.#source.titles();
    const factor = edgeScrollFactor(drag.last.x, left, rect.right - left, DRAG_SETTINGS.scrollEdge);
    if (factor === 0) {
      drag.owed = 0;
    } else {
      const distance = drag.owed + (factor * DRAG_SETTINGS.scrollMaxSpeed * elapsed) / 1000;
      const whole = Math.trunc(distance);
      drag.owed = distance - whole;
      const before = scroller.scrollLeft;
      if (whole !== 0) scroller.scrollLeft = before + whole;
      if (scroller.scrollLeft !== before) this.#update();
    }
    drag.frame = requestAnimationFrame(this.#frame);
  };

  /** The label goes by the pointer, `at` days from FROM. */
  #place(at: number, point: Point): void {
    const scroller = this.#source.scroller();
    const axis = this.#source.axis();
    if (!scroller) return;
    const box = scroller.getBoundingClientRect();
    const left = box.left + this.#source.titles();
    const part = Math.min(1, Math.max(0, (point.x - left) / Math.max(1, box.right - left)));
    this.label = { x: (at - daysBetween(FROM, axis.first)) * axis.dayWidth, part };
  }

  #end(): void {
    const drag = this.#drag;
    this.#drag = undefined;
    if (drag?.frame !== undefined) cancelAnimationFrame(drag.frame);
    this.preview = undefined;
    this.reach = undefined;
    this.label = undefined;
  }
}
