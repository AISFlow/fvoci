/**
 * What the calendar and the Gantt chart write to the board: a card's date
 * moved to another day, or one end of it, by a picker of days or a drag. One change of one property, so
 * that one undo takes it back (`BoardStore.setCardProperty`).
 */
import { movedValue, resizedValue, type BarGrip, type ValueEnd } from '$lib/logic/calendar.ts';
import type { CalendarDate, DateValue } from '$lib/logic/dates.ts';
import {
  cardProperty,
  formatDateValue,
  parseDateValue,
  type Card,
  type PropertyTemplate,
} from '$lib/logic/wire.ts';
import type { BoardStore } from '$lib/state/index.ts';

/**
 * Writes the card's value of `template` as `change` makes it from the value
 * it has, if the wire can carry it. True once the board has kept it: a change the backend refuses is
 * reported and put back by the board, and gives false. The board shows the
 * new value at once, before the backend has answered.
 */
async function writeDate(
  board: BoardStore,
  card: Card,
  template: PropertyTemplate,
  change: (value: DateValue) => DateValue,
): Promise<boolean> {
  const next = formatDateValue(change(parseDateValue(cardProperty(card, template.id))));
  // A moment the wire cannot carry (before 1970 or after 9999 in UTC) does
  // not read back: nothing is written, and the card stays where it was.
  const readBack = formatDateValue(parseDateValue(next));
  if (readBack?.from !== next?.from || readBack?.to !== next?.to) return false;
  await board.setCardProperty(card.id, template.id, next);
  const now = board.card(card.id);
  if (!now) return false;
  // Both read the same way, whatever order the backend gave the ends in.
  const kept = formatDateValue(parseDateValue(cardProperty(now, template.id)));
  return kept?.from === next?.from && kept?.to === next?.to;
}

/**
 * Moves the card's value of `template` (a date property) so that it starts
 * on `day`, keeping its length and time of day (`movedValue`); a card with no
 * value gets that day.
 */
export function moveCard(
  board: BoardStore,
  card: Card,
  template: PropertyTemplate,
  day: CalendarDate,
  timeZone: string,
): Promise<boolean> {
  return writeDate(board, card, template, (value) => movedValue(value, day, timeZone));
}

/** Moves the first or the last day of the card's value of `template` to `day` (`resizedValue`). */
export function resizeCard(
  board: BoardStore,
  card: Card,
  template: PropertyTemplate,
  end: ValueEnd,
  day: CalendarDate,
  timeZone: string,
): Promise<boolean> {
  return writeDate(board, card, template, (value) => resizedValue(value, end, day, timeZone));
}

/**
 * Writes a dragged card's days to it: the value moved whole to the first day
 * (`moveCard`), or the end dragged moved to its day (`resizeCard`).
 */
export function writeDays(
  board: BoardStore,
  card: Card,
  template: PropertyTemplate,
  grip: BarGrip,
  days: { first: CalendarDate; last: CalendarDate },
  timeZone: string,
): Promise<boolean> {
  if (grip === 'move') return moveCard(board, card, template, days.first, timeZone);
  const day = grip === 'start' ? days.first : days.last;
  return resizeCard(board, card, template, grip, day, timeZone);
}
