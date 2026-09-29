// Calendar dates and wall-clock times in a user's IANA time zone, for the
// session's `timezone` (an unknown zone falls back to FALLBACK_TZ, as
// lib/datetime.ts does).
import { FALLBACK_TZ } from "@/lib/datetime";
import type { IsoDate } from "@/lib/iso-date";

const DAY_MS = 86_400_000;

const formatters = new Map<string, Intl.DateTimeFormat>();

function formatter(timeZone: string): Intl.DateTimeFormat {
  let f = formatters.get(timeZone);
  if (!f) {
    const options: Intl.DateTimeFormatOptions = {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hourCycle: "h23",
    };
    try {
      f = new Intl.DateTimeFormat("en-US", { ...options, timeZone });
    } catch {
      f = new Intl.DateTimeFormat("en-US", { ...options, timeZone: FALLBACK_TZ });
    }
    formatters.set(timeZone, f);
  }
  return f;
}

/** The wall-clock time at `utcMs` in `timeZone`, as a UTC-based millisecond count. */
function wallMs(utcMs: number, timeZone: string): number {
  const parts = formatter(timeZone).formatToParts(new Date(utcMs));
  const n = (type: Intl.DateTimeFormatPartTypes) => Number(parts.find((p) => p.type === type)?.value);
  const ms = ((utcMs % 1000) + 1000) % 1000;
  return Date.UTC(n("year"), n("month") - 1, n("day"), n("hour"), n("minute"), n("second"), ms);
}

/** The calendar date at `utcMs` in `timeZone`. */
export function dateInZone(utcMs: number, timeZone: string): IsoDate {
  return new Date(wallMs(utcMs, timeZone)).toISOString().slice(0, 10);
}

/**
 * `iso` moved by `days` calendar days in `timeZone`, keeping its wall-clock
 * time there (seconds and milliseconds included). A wall time that falls in
 * a daylight-saving gap moves forward by the gap and an ambiguous one takes
 * the earlier instant, like Temporal's "compatible" disambiguation. Returns
 * an RFC 3339 UTC instant with milliseconds.
 */
export function shiftInstantDays(iso: string, days: number, timeZone: string): string {
  const utcMs = Date.parse(iso);
  if (Number.isNaN(utcMs)) throw new RangeError(`invalid instant: ${iso}`);
  const target = wallMs(utcMs, timeZone) + days * DAY_MS;
  const offsets = [...new Set([-DAY_MS, 0, DAY_MS].map((d) => wallMs(target + d, timeZone) - (target + d)))];
  const matches = offsets
    .map((offset) => target - offset)
    .filter((candidate) => wallMs(candidate, timeZone) === target)
    .sort((a, b) => a - b);
  // In a gap no instant shows `target`; the offset before the transition
  // lands the same distance past it.
  const before = wallMs(target - DAY_MS, timeZone) - (target - DAY_MS);
  return new Date(matches[0] ?? target - before).toISOString();
}
