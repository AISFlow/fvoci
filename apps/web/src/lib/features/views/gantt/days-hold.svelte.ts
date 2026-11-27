/**
 * Keeps the days in sight where they are when the axis of a Gantt chart
 * changes under them: the zoom, or its first day (a card dated earlier than
 * any before, another "display by", a filter).
 *
 * Where a box is scrolled to is the page's and not ours, so this is an
 * effect: before the page is drawn again the place of a day is read off the
 * old axis, and once it is drawn the box is scrolled to bring that day to
 * the same place on the new one. Today is the day held when its line is in
 * sight (it stays where it is); otherwise the day in the middle of what is
 * in sight.
 *
 * The day is held again at the next change until the reader scrolls: at a
 * far zoom the whole chart may be in sight and the box not scrolled at all,
 * and zooming back in must come back to where the reader was, not to
 * whatever then lay in the middle.
 */
import { tick, untrack } from 'svelte';

import { calendarDate, daysBetween, type CalendarDate } from '$lib/logic/dates.ts';
import type { Axis } from '$lib/logic/gantt.ts';

export interface HoldSource {
  axis: () => Axis;
  /** Today on the reader's calendar. */
  today: () => CalendarDate;
  /** The box that scrolls; undefined while it is not drawn. */
  scroller: () => HTMLElement | undefined;
  /** How wide the days in sight are, in pixels: the box less the column of titles. */
  room: () => number;
  /** The size of 1rem in pixels. */
  rem: () => number;
}

/** Days are counted from here, with their fraction: any day does. */
const FROM = calendarDate(2000, 1, 1);

export class DaysHold {
  readonly #source: HoldSource;
  /** The axis the page shows; the one before it, while a new one is being drawn. */
  #drawn: Axis | undefined;
  /** The day held last, how far into the days in sight, and where the box was scrolled to for it. */
  #held: { day: number; at: number; left: number } | undefined;

  /** To be made while a component is: it follows the axis from then on. */
  constructor(source: HoldSource) {
    this.#source = source;
    $effect.pre(() => {
      const next = source.axis();
      const before = this.#drawn;
      this.#drawn = next;
      const scroller = source.scroller();
      if (!scroller || !before) return;
      if (before.first === next.first && before.dayWidth === next.dayWidth) return;
      const left = scroller.scrollLeft;
      const held = this.#held;
      const hold =
        held && Math.abs(held.left - left) < 1 ? held : untrack(() => this.#toHold(before, left));
      void tick().then(() => {
        const rem = source.rem();
        const x = (hold.day - daysBetween(FROM, next.first)) * next.dayWidth * rem;
        scroller.scrollLeft = Math.max(0, x - hold.at);
        this.#held = { day: hold.day, at: hold.at, left: scroller.scrollLeft };
      });
    });
  }

  /** The reader asked for a day themselves ("Today"): no earlier one is held any more. */
  release(): void {
    this.#held = undefined;
  }

  #toHold(before: Axis, left: number): { day: number; at: number } {
    const { room, rem, today } = this.#source;
    const from = daysBetween(FROM, before.first);
    // Today on the axis that is still drawn, not on the one that is coming.
    const todayDays = daysBetween(before.first, today()) + 0.5;
    const todayAt = todayDays * before.dayWidth * rem() - left;
    if (todayAt >= 0 && todayAt <= room()) return { day: from + todayDays, at: todayAt };
    return { day: from + (left + room() / 2) / rem() / before.dayWidth, at: room() / 2 };
  }
}
