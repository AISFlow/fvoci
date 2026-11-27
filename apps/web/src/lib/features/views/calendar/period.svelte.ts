/**
 * What a calendar view is showing: a month or a week, the
 * month in view, and the day picked. Kept above the parts that draw them, so
 * that a window that changes size and swaps the grid of bars for the small
 * grid or the list of a week does not take them along.
 *
 * Month or week is remembered on the device, for each view: it is the
 * reader's way of looking, not the view's, and the server keeps no
 * field for it. The month and the day are the screen's only: a new load
 * starts at today.
 */
import {
  firstOf,
  inMonth,
  monthOf,
  shiftMonth,
  weekOf,
  weeksOfMonth,
  type Month,
} from '$lib/logic/calendar.ts';
import { addDays, type CalendarDate } from '$lib/logic/dates.ts';
import type { Weekday } from '$lib/logic/regional.ts';

export type Scale = 'month' | 'week';

const STORAGE_PREFIX = 'fvoci.calendar.scale.';

/** Month or week, for each view, as this device remembers it. */
class Scales {
  readonly #read = $state<Record<string, Scale>>({});

  get(viewId: string): Scale {
    const known = this.#read[viewId];
    if (known) return known;
    let stored: string | null = null;
    try {
      stored = localStorage.getItem(STORAGE_PREFIX + viewId);
    } catch {
      // Nothing remembered: a month.
    }
    return stored === 'week' ? 'week' : 'month';
  }

  set(viewId: string, scale: Scale): void {
    this.#read[viewId] = scale;
    try {
      localStorage.setItem(STORAGE_PREFIX + viewId, scale);
    } catch {
      // Not remembered: it still holds for this visit.
    }
  }
}

/** Month or week of every calendar view, as this device remembers them. */
export const scales = new Scales();

export class Period {
  readonly #viewId: string;
  month = $state<Month>({ year: 1970, month: 1 });
  selected = $state<CalendarDate>('1970-01-01' as CalendarDate);

  constructor(viewId: string, today: CalendarDate) {
    this.#viewId = viewId;
    this.month = monthOf(today);
    this.selected = today;
  }

  get scale(): Scale {
    return scales.get(this.#viewId);
  }

  set scale(scale: Scale) {
    scales.set(this.#viewId, scale);
    // A month shown again is the month of the day picked in the week.
    this.month = monthOf(this.selected);
  }

  /** The weeks drawn: those of the month, or the one of the day picked. */
  weeks(weekStart: Weekday): CalendarDate[][] {
    return this.scale === 'week'
      ? [weekOf(this.selected, weekStart)]
      : weeksOfMonth(this.month, weekStart);
  }

  /** Picks a day; a day of another month brings that month into view. */
  select(date: CalendarDate): void {
    this.selected = date;
    if (!inMonth(date, this.month)) this.month = monthOf(date);
  }

  /** A month or a week on or back. In a month, the day picked is today in the month of today, else the first. */
  step(by: number, today: CalendarDate): void {
    if (this.scale === 'week') {
      this.select(addDays(this.selected, by * 7));
      return;
    }
    this.month = shiftMonth(this.month, by);
    this.selected = inMonth(today, this.month) ? today : firstOf(this.month);
  }

  goTo(today: CalendarDate): void {
    this.month = monthOf(today);
    this.selected = today;
  }
}
