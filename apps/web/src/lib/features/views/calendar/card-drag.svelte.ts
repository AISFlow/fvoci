/**
 * Dragging cards on the calendar: a bar of the grid (a month, or a week of
 * bars) to another day, an end of a bar by its grip to make it longer or
 * shorter, and a card without a date from its column onto a day, which
 * gives it that day.
 *
 * A mouse takes a card at the press, and a press let go where it was is a
 * click, which opens the card (`pointerDrag`, `immediate`). A finger rests
 * on the card first, so that a finger that moves at once scrolls; on the
 * grid of a month, where a bar is a picture over its cell, a finger resting
 * on a bar takes the bar, and a tap is still the cell's. Escape does not
 * cancel: a card let go on the day it came from, or anywhere that is not a
 * day, changes nothing.
 *
 * While a card is dragged its days are marked on the grid (`preview`) and a
 * chip with its title and days follows the pointer; the board changes only
 * when it is let go, in one step that one undo takes back. Any element with
 * `data-day` under the pointer is a day, the small grid's buttons included.
 * The column of cards without a date lies over the grid at its right: while
 * a card is dragged it fades and the pointer goes through it to the days.
 */
import { pointerDrag, type PointerDragSession } from '$lib/drag/pointer-drag.ts';
import { draggedDays, type BarGrip } from '$lib/logic/calendar.ts';
import { daysBetween, type CalendarDate } from '$lib/logic/dates.ts';
import type { Days } from '$lib/logic/dependencies.ts';
import type { Point } from '$lib/logic/drop-target.ts';

export interface CardDragSource {
  /** The days of a card; none for a card without a date. */
  days: (cardId: string) => Days | undefined;
  /** Cards may be dragged: by someone who may edit, where the calendar is placed by a date to set. */
  enabled: () => boolean;
  /** A card was let go on other days than its own. */
  ondrop: (cardId: string, grip: BarGrip, days: Days) => void;
}

/** The card being dragged, and the days it would go to; none while it is not over a day. */
export interface CardPreview {
  id: string;
  grip: BarGrip;
  days: Days | undefined;
}

/** The day under a point: the topmost element there that is a day, unless something lies over it. */
function dayAt(root: HTMLElement, point: Point): CalendarDate | undefined {
  const hits = document.elementsFromPoint(point.x, point.y);
  const top = hits[0];
  // Over the column of cards without a date, or anything else on top of the calendar.
  if (!top || !root.contains(top) || top.closest('[data-testid="undated-panel"]')) return undefined;
  for (const hit of hits) {
    const day = hit.closest<HTMLElement>('[data-day]')?.dataset.day;
    if (day !== undefined && root.contains(hit)) return day as CalendarDate;
  }
  return undefined;
}

/** The bar under a point in a week of the grid, where bars are pictures the pointer goes through. */
function barAt(week: Element, point: Point): HTMLElement | undefined {
  return [...week.querySelectorAll<HTMLElement>('[data-bar]')].find((bar) => {
    const box = bar.getBoundingClientRect();
    return (
      point.x >= box.left && point.x <= box.right && point.y >= box.top && point.y <= box.bottom
    );
  });
}

/** Which grip of a bar a point is on, if any. */
function gripAt(bar: HTMLElement, point: Point): BarGrip | undefined {
  for (const grip of bar.querySelectorAll<HTMLElement>('[data-grip]')) {
    const box = grip.getBoundingClientRect();
    if (point.x >= box.left && point.x <= box.right) {
      return grip.dataset.grip === 'start' ? 'start' : 'end';
    }
  }
  return undefined;
}

export class CardDrag {
  readonly #source: CardDragSource;
  preview = $state.raw<CardPreview | undefined>(undefined);
  /** Where the pointer is while a card is dragged: the chip with its title follows it. */
  at = $state.raw<Point | undefined>(undefined);
  #root: HTMLElement | undefined;

  constructor(source: CardDragSource) {
    this.#source = source;
  }

  /** The box of the calendar, the column of cards without a date in it. For `{@attach}`. */
  readonly attach = (root: HTMLElement): (() => void) => {
    this.#root = root;
    const handle = pointerDrag(root, this.#press, { immediate: true, escapeCancels: false });
    return () => {
      handle.destroy();
      this.#root = undefined;
    };
  };

  /** The card a press is on, and what of it. */
  #pressed(event: PointerEvent): { id: string; grip: BarGrip } | undefined {
    const target = event.target instanceof Element ? event.target : null;
    if (!target || target.closest('[data-card-menu], [data-new-card], [data-more]')) {
      return undefined;
    }
    const point = { x: event.clientX, y: event.clientY };
    const line = target.closest<HTMLElement>('[data-testid="undated-list"] [data-card]');
    if (line?.dataset.card !== undefined) return { id: line.dataset.card, grip: 'move' };
    const week = target.closest('[data-week]');
    const bar =
      target.closest<HTMLElement>('[data-bar]') ?? (week ? barAt(week, point) : undefined);
    const id = bar?.dataset.bar;
    if (!bar || id === undefined) return undefined;
    return { id, grip: gripAt(bar, point) ?? 'move' };
  }

  readonly #press = (event: PointerEvent): PointerDragSession | undefined => {
    const root = this.#root;
    if (!root || !this.#source.enabled()) return undefined;
    const pressed = this.#pressed(event);
    if (!pressed) return undefined;
    const { id, grip } = pressed;
    const days = this.#source.days(id);
    const shownAtOnce = event.pointerType !== 'mouse';
    /** The day under the press: a bar keeps the place it was taken by. */
    let origin: CalendarDate | undefined;
    const show = (at: Point): void => {
      const day = dayAt(root, at);
      let next: Days | undefined;
      if (!days) next = day ? { first: day, last: day } : undefined;
      else if (day && origin) next = draggedDays(days, grip, daysBetween(origin, day));
      else if (day) next = days;
      this.at = at;
      const shown = this.preview;
      // The marks of the days change only with the days.
      if (shown && shown.days?.first === next?.first && shown.days?.last === next?.last) return;
      this.preview = { id, grip, days: next };
    };
    return {
      start: (point) => {
        origin = dayAt(root, point);
        if (shownAtOnce) show(point);
      },
      move: show,
      drop: (point) => {
        show(point);
        const shown = this.preview?.days;
        this.preview = undefined;
        this.at = undefined;
        if (!shown || (days?.first === shown.first && days.last === shown.last)) return;
        this.#source.ondrop(id, grip, shown);
      },
      cancel: () => {
        this.preview = undefined;
        this.at = undefined;
      },
    };
  };
}
