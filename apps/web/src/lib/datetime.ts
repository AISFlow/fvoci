// Adapted from source apps/web/src/lib/datetime.ts (formatInstant/formatDateKo).
export const FALLBACK_TZ = "Asia/Seoul";

export function formatInstant(
  value: string | Date,
  timeZone: string,
  options: Intl.DateTimeFormatOptions,
): string {
  const date = typeof value === "string" ? new Date(value) : value;
  try {
    return date.toLocaleString("ko-KR", { hour12: false, timeZone, ...options });
  } catch {
    return date.toLocaleString("ko-KR", { hour12: false, timeZone: FALLBACK_TZ, ...options });
  }
}

export function formatDateKo(value: string | Date, timeZone: string = FALLBACK_TZ): string {
  return formatInstant(value, timeZone, { year: "numeric", month: "long", day: "numeric" });
}

// Source apps/web/src/lib/datetime.ts: wall-clock `datetime-local` values in a
// user time zone to and from UTC instants.
function tzOffsetMs(utcMs: number, timeZone: string): number {
  const opts: Intl.DateTimeFormatOptions = {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
    hourCycle: "h23",
  };
  let parts: Intl.DateTimeFormatPart[];
  try {
    parts = new Intl.DateTimeFormat("en-US", opts).formatToParts(new Date(utcMs));
  } catch {
    parts = new Intl.DateTimeFormat("en-US", { ...opts, timeZone: FALLBACK_TZ }).formatToParts(
      new Date(utcMs),
    );
  }
  const n = (type: Intl.DateTimeFormatPartTypes): number =>
    Number(parts.find((p) => p.type === type)?.value);
  return Date.UTC(n("year"), n("month") - 1, n("day"), n("hour"), n("minute"), n("second")) - utcMs;
}

export function isoToDatetimeLocalInTimeZone(iso: string, timeZone: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return "";
  const opts: Intl.DateTimeFormatOptions = {
    timeZone,
    year: "numeric",
    month: "2-digit",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    hourCycle: "h23",
  };
  let parts: Intl.DateTimeFormatPart[];
  try {
    parts = new Intl.DateTimeFormat("en-US", opts).formatToParts(date);
  } catch {
    parts = new Intl.DateTimeFormat("en-US", { ...opts, timeZone: FALLBACK_TZ }).formatToParts(date);
  }
  const n = (type: Intl.DateTimeFormatPartTypes): string =>
    parts.find((p) => p.type === type)?.value ?? "";
  return `${n("year")}-${n("month")}-${n("day")}T${n("hour")}:${n("minute")}`;
}

/** "" for a malformed value or a wall-clock time skipped by a DST gap; an
 * overlap picks the earlier instant. */
export function datetimeLocalInTimeZoneToIso(local: string, timeZone: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(local);
  if (!m) return "";
  const utcMs = Date.UTC(Number(m[1]), Number(m[2]) - 1, Number(m[3]), Number(m[4]), Number(m[5]));
  const candidates = [
    ...new Set([-86400000, 0, 86400000].map((delta) => utcMs - tzOffsetMs(utcMs + delta, timeZone))),
  ].sort((a, b) => a - b);
  const match = candidates.find(
    (ms) => isoToDatetimeLocalInTimeZone(new Date(ms).toISOString(), timeZone) === local,
  );
  return match === undefined ? "" : new Date(match).toISOString();
}

export function durationSecondsBetween(startedAtIso: string, endedAtIso: string): number {
  return (Date.parse(endedAtIso) - Date.parse(startedAtIso)) / 1000;
}
