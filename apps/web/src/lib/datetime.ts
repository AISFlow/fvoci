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
