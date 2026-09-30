/** A calendar date, `YYYY-MM-DD`. */
export type IsoDate = string;

const ISO_RE = /^\d{4}-\d{2}-\d{2}$/;
const MS_PER_DAY = 86_400_000;

export function isIsoDate(v: string): v is IsoDate {
  if (!ISO_RE.test(v)) return false;
  return toEpochDay(v) !== null;
}

export function toEpochDay(d: IsoDate): number | null {
  if (!ISO_RE.test(d)) return null;
  const ms = Date.parse(`${d}T00:00:00Z`);
  if (Number.isNaN(ms)) return null;
  if (fromEpochDay(Math.floor(ms / MS_PER_DAY)) !== d) return null;
  return Math.floor(ms / MS_PER_DAY);
}

export function fromEpochDay(day: number): IsoDate {
  const iso = new Date(day * MS_PER_DAY).toISOString();
  return iso.slice(0, 10);
}

export function daysBetween(a: IsoDate, b: IsoDate): number | null {
  const x = toEpochDay(a);
  const y = toEpochDay(b);
  if (x === null || y === null) return null;
  return y - x;
}

export function addDays(d: IsoDate, n: number): IsoDate | null {
  const day = toEpochDay(d);
  if (day === null) return null;
  return fromEpochDay(day + n);
}

export function dayOfWeek(d: IsoDate): number | null {
  const day = toEpochDay(d);
  if (day === null) return null;
  return (((day + 4) % 7) + 7) % 7;
}

export function eachDay(start: IsoDate, end: IsoDate): IsoDate[] {
  const s = toEpochDay(start);
  const e = toEpochDay(end);
  if (s === null || e === null || e < s) return [];
  const out: IsoDate[] = [];
  for (let d = s; d <= e; d++) out.push(fromEpochDay(d));
  return out;
}
