/**
 * Moments for tests, said the way a person would: "9 in the morning of
 * 5 March in Seoul". A test never builds a moment from the time zone of the
 * machine it runs on, so its result is the same wherever it runs.
 */
import { CalendarDateTime, toZoned } from '@internationalized/date';

/** The moment the clocks of `timeZone` show the given date and time. */
export function at(
  timeZone: string,
  year: number,
  month: number,
  day: number,
  hour = 0,
  minute = 0,
  second = 0,
): number {
  return toZoned(new CalendarDateTime(year, month, day, hour, minute, second), timeZone)
    .toDate()
    .getTime();
}

/** Time zones worth trying: both edges of the date line, clocks that change, and odd offsets. */
export const ZONES = [
  'UTC',
  'Asia/Seoul',
  'Pacific/Kiritimati', // UTC+14, the first to see a day
  'Pacific/Pago_Pago', // UTC-11
  'Etc/GMT+12', // UTC-12, the last to see a day
  'America/New_York', // clocks change
  'Europe/Lisbon', // clocks change
  'Australia/Lord_Howe', // clocks change by half an hour
  'Asia/Kathmandu', // UTC+5:45
  'America/Havana', // clocks change at midnight
] as const;
