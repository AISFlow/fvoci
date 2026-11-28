/** How a card repeats, in words: "Weekly", "every 3 days · from the day it is done". */
import { m } from '$lib/i18n/paraglide/messages.js';
import type { Recurrence, RecurrenceUnit } from '$lib/logic/recurrence.ts';

const ONCE: Record<RecurrenceUnit, () => string> = {
  day: () => m.property_date_repeat_daily(),
  week: () => m.property_date_repeat_weekly(),
  month: () => m.property_date_repeat_monthly(),
  year: () => m.property_date_repeat_yearly(),
};

const EVERY: Record<RecurrenceUnit, (inputs: { count: number }) => string> = {
  day: (inputs) => m.card_repeats_every_day(inputs),
  week: (inputs) => m.card_repeats_every_week(inputs),
  month: (inputs) => m.card_repeats_every_month(inputs),
  year: (inputs) => m.card_repeats_every_year(inputs),
};

export function recurrenceText(recurrence: Recurrence): string {
  const how =
    recurrence.every === 1
      ? ONCE[recurrence.unit]()
      : EVERY[recurrence.unit]({ count: recurrence.every });
  return recurrence.anchor === 'done' ? m.card_repeats_from_done({ how }) : how;
}

/** The name of a unit in the field that picks it. */
export const UNIT_NAMES: Record<RecurrenceUnit, () => string> = {
  day: () => m.property_date_repeat_unit_day(),
  week: () => m.property_date_repeat_unit_week(),
  month: () => m.property_date_repeat_unit_month(),
  year: () => m.property_date_repeat_unit_year(),
};
