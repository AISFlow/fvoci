/**
 * The days of a diary written in a row: the runs its written days make, and
 * how many days the reader has written in a row now and at most.
 *
 * Today is always an argument, the day of the reader's device
 * (`features/diary/today.svelte.ts`): the server says which days are written,
 * never which day it is.
 */
import { addDays, daysBetween, type CalendarDate } from './dates.ts';

/** Days written one after the other; `days` counts both ends. */
export interface Run {
  from: CalendarDate;
  to: CalendarDate;
  days: number;
}

export interface Streak {
  /** The days written in a row up to today, or up to yesterday while today is not written yet. */
  current: number;
  /** The most days the diary has written in a row. */
  longest: number;
}

/** The runs written days make, the earliest first; the days come in any order, each once. */
export function runsOf(days: readonly CalendarDate[]): Run[] {
  const runs: Run[] = [];
  for (const day of [...days].sort()) {
    const last = runs.at(-1);
    if (last && addDays(last.to, 1) === day) {
      last.to = day;
      last.days += 1;
    } else {
      runs.push({ from: day, to: day, days: 1 });
    }
  }
  return runs;
}

/**
 * How many days are written in a row on `today`, from the runs of the diary
 * (the earliest first). A day not written yet today breaks nothing: the run
 * that ends yesterday is still going. Days written after today (on a device
 * where they have come) count from the day they come here, in either number.
 */
export function streakOf(runs: readonly Run[], today: CalendarDate): Streak {
  const yesterday = addDays(today, -1);
  let current = 0;
  let longest = 0;
  for (const run of runs) {
    const reached = run.to >= today ? today : run.to;
    if (run.from > reached) continue;
    const days = daysBetween(run.from, reached) + 1;
    longest = Math.max(longest, days);
    if (reached >= yesterday) current = days;
  }
  return { current, longest };
}
