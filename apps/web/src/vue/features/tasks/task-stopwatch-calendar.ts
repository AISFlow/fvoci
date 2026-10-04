import { isoToDatetimeLocalInTimeZone } from "@/lib/datetime";

/** Calendar labels only: UTC arithmetic on these labels never measures time.
 * The server splits actual instant intervals at the selected zone's midnights. */
export function timerCalendarRange(iso: string, timeZone: string, weekStartsOn: 0 | 1) {
  const today = isoToDatetimeLocalInTimeZone(iso, timeZone).slice(0, 10);
  const date = new Date(`${today}T00:00:00Z`);
  const offset = (date.getUTCDay() - weekStartsOn + 7) % 7;
  date.setUTCDate(date.getUTCDate() - offset);
  const weekFrom = date.toISOString().slice(0, 10);
  date.setUTCDate(date.getUTCDate() + 6);
  return { today, weekFrom, weekTo: date.toISOString().slice(0, 10) };
}
