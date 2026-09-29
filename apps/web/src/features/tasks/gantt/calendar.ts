import { addDays, dayOfWeek, eachDay } from "@/lib/iso-date";
import type { IsoDate, WorkCalendar } from "./types";

export const DEFAULT_CALENDAR: WorkCalendar = {
	weekend: [0, 6],
	holidays: new Set<IsoDate>(),
};

export function makeCalendar(
	holidays: Iterable<IsoDate>,
	weekend: readonly number[] = [0, 6],
): WorkCalendar {
	return { weekend: [...weekend], holidays: new Set(holidays) };
}

export function isWorkingDay(d: IsoDate, cal: WorkCalendar): boolean {
	const dow = dayOfWeek(d);
	if (dow === null) return false;
	if (cal.weekend.includes(dow)) return false;
	return !cal.holidays.has(d);
}

export function workingDaysBetween(
	start: IsoDate,
	end: IsoDate,
	cal: WorkCalendar,
): number {
	let n = 0;
	for (const d of eachDay(start, end)) {
		if (isWorkingDay(d, cal)) n++;
	}
	return n;
}

export function offDutyDays(
	start: IsoDate,
	end: IsoDate,
	cal: WorkCalendar,
): IsoDate[] {
	return eachDay(start, end).filter((d) => !isWorkingDay(d, cal));
}

export function addWorkingDays(
	d: IsoDate,
	n: number,
	cal: WorkCalendar,
): IsoDate | null {
	if (n === 0) return d;
	if (n < 0) return null;
	let remaining = n;
	let cur: IsoDate = d;
	let guard = 0;
	while (remaining > 0) {
		const next = addDays(cur, 1);
		if (next === null) return null;
		cur = next;
		if (isWorkingDay(cur, cal)) remaining--;
		if (++guard > 800) return null;
	}
	return cur;
}
